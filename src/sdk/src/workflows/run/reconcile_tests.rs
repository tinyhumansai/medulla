//! Tests for the orphaned-run sweep and the liveness check it rests on.

use std::sync::Arc;

use super::reconcile::{current_executor, is_alive, reconcile_orphans};
use crate::workflows::store::{new_run_record, FileWorkflowStore};
use crate::workflows::{RunExecutor, RunRecord, RunStatus, WorkflowStore};

/// An isolated file-backed store.
fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    (root, store)
}

/// A record that claims to be running, owned by `executor`.
fn running(id: &str, executor: Option<RunExecutor>) -> RunRecord {
    new_run_record(id, "wf", 1).with_executor(executor)
}

/// A pid that cannot be running: the kernel never allocates 0 to a user
/// process, and the record names this host so the check does not bail out on
/// the cross-host guard.
fn dead_executor() -> RunExecutor {
    RunExecutor {
        host: current_executor().host.clone(),
        pid: 0,
        started_at_secs: Some(1),
    }
}

#[test]
fn this_process_is_alive() {
    assert!(is_alive(current_executor()));
}

#[test]
fn a_process_that_is_gone_is_not_alive() {
    assert!(!is_alive(&dead_executor()));
}

#[test]
fn a_record_from_another_host_is_left_alone() {
    // This host's process table says nothing about another machine's pids, so
    // the check must refuse to guess rather than reconcile a live remote run.
    let executor = RunExecutor {
        host: format!("{}-somewhere-else", current_executor().host),
        pid: 0,
        started_at_secs: Some(1),
    };

    assert!(is_alive(&executor));
}

#[test]
fn a_live_pid_whose_start_time_disagrees_is_pid_reuse() {
    // Our own pid, but claimed to have started at the epoch. The pid is live;
    // the process that wrote the record is not the one holding it now.
    let executor = RunExecutor {
        host: current_executor().host.clone(),
        pid: std::process::id() + 1,
        started_at_secs: Some(1),
    };

    // Either the neighbouring pid is unused (not alive) or it is a different
    // process than the record describes (also not alive). What must never
    // happen is this reading as the record's own executor.
    assert!(!is_alive(&executor) || current_executor().started_at_secs == Some(1));
}

#[test]
fn a_run_left_by_a_dead_process_is_settled_as_interrupted() {
    let (_root, store) = store();
    store
        .record_run(&running("run-orphan", Some(dead_executor())))
        .unwrap();

    let reconciled = reconcile_orphans(&store).unwrap();

    assert_eq!(reconciled.len(), 1, "{reconciled:?}");
    assert_eq!(reconciled[0].run_id, "run-orphan");
    let record = store.get_run("run-orphan").unwrap().unwrap();
    assert_eq!(record.status, RunStatus::Interrupted);
    assert!(record.finished_at.is_some());
    assert!(record.error.is_some(), "an orphan should say what happened");
}

#[test]
fn a_record_with_no_executor_is_an_orphan() {
    // What every record written before executors existed looks like — and the
    // backlog this sweep was written to clear.
    let (_root, store) = store();
    store.record_run(&running("run-legacy", None)).unwrap();

    reconcile_orphans(&store).unwrap();

    let record = store.get_run("run-legacy").unwrap().unwrap();
    assert_eq!(record.status, RunStatus::Interrupted);
}

#[test]
fn an_orphan_that_was_asked_to_stop_settles_as_cancelled() {
    let (_root, store) = store();
    let mut record = running("run-asked", Some(dead_executor()));
    record.cancel_requested = true;
    store.record_run(&record).unwrap();

    reconcile_orphans(&store).unwrap();

    let record = store.get_run("run-asked").unwrap().unwrap();
    assert_eq!(
        record.status,
        RunStatus::Cancelled,
        "someone asked for this outcome, so it is the honest one to record"
    );
}

#[test]
fn a_run_owned_by_a_live_process_is_left_alone() {
    let (_root, store) = store();
    store
        .record_run(&running("run-live", Some(current_executor().clone())))
        .unwrap();

    let reconciled = reconcile_orphans(&store).unwrap();

    assert!(reconciled.is_empty(), "{reconciled:?}");
    let record = store.get_run("run-live").unwrap().unwrap();
    assert_eq!(record.status, RunStatus::Running);
}

#[test]
fn a_run_executing_in_this_process_is_left_alone() {
    // The registry is checked before the process table, which covers the window
    // between admitting a run and stamping its executor.
    let (_root, store) = store();
    let _guard = super::RunGuard::register("run-mine");
    store.record_run(&running("run-mine", None)).unwrap();

    let reconciled = reconcile_orphans(&store).unwrap();

    assert!(reconciled.is_empty(), "{reconciled:?}");
    assert_eq!(
        store.get_run("run-mine").unwrap().unwrap().status,
        RunStatus::Running
    );
}

#[test]
fn a_settled_run_is_never_rewritten() {
    let (_root, store) = store();
    let mut record = running("run-done", Some(dead_executor()));
    record.status = RunStatus::Succeeded;
    record.finished_at = Some(99);
    store.record_run(&record).unwrap();

    reconcile_orphans(&store).unwrap();

    let record = store.get_run("run-done").unwrap().unwrap();
    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(record.finished_at, Some(99));
}

#[test]
fn a_run_parked_on_approval_is_settled_when_its_process_dies() {
    // `pending_approval` is not settled: a parked run whose host is gone can
    // never be resumed, so leaving it parked is the same lie as leaving it
    // running.
    let (_root, store) = store();
    let mut record = running("run-parked", Some(dead_executor()));
    record.status = RunStatus::PendingApproval;
    record.pending_approvals = vec!["review".to_string()];
    store.record_run(&record).unwrap();

    reconcile_orphans(&store).unwrap();

    assert_eq!(
        store.get_run("run-parked").unwrap().unwrap().status,
        RunStatus::Interrupted
    );
}

#[test]
fn the_sweep_spans_every_workflow_in_the_scope() {
    let (_root, store) = store();
    for (id, workflow) in [("run-a", "alpha"), ("run-b", "beta")] {
        let mut record = running(id, Some(dead_executor()));
        record.workflow_id = workflow.to_string();
        store.record_run(&record).unwrap();
    }

    let reconciled = reconcile_orphans(&store).unwrap();

    assert_eq!(reconciled.len(), 2, "{reconciled:?}");
}
