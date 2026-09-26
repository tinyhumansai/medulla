//! Unit tests for the run-detail join.
//!
//! Two halves worth checking separately: the parsing that turns a fleet task id
//! back into a workflow route, which is the whole correlation, and the assembly
//! that decides what to say when one half of the answer is missing.

use std::sync::Arc;

use serde_json::json;

use serde_json::Value;

use crate::control_socket::{ControlError, FleetWorker, ToolFamilies};
use crate::mcp::{FleetBackend, McpSession, OfflineFleet};
use crate::workflows::ops::StepDetail;
use crate::workflows::run::{self, InFlightDispatch};
use crate::workflows::{FileWorkflowStore, WorkflowStore};

use super::detail::{
    detail, dispatch_prefix, dispatches_here, dispatches_in, merge, note, split_suffix,
};
use super::types::Standing;

/// A fleet that answers `worker.list` with a roster the test wrote.
struct StubFleet {
    hello: crate::control_socket::Hello,
    workers: Vec<FleetWorker>,
}

#[async_trait::async_trait]
impl FleetBackend for StubFleet {
    fn hello(&self) -> Option<&crate::control_socket::Hello> {
        Some(&self.hello)
    }

    async fn call(&self, op: &str, _params: Value) -> Result<Value, ControlError> {
        assert_eq!(op, "worker.list", "run detail asks for the roster only");
        Ok(json!({ "workers": self.workers, "defaultWorker": "laptop" }))
    }
}

/// A handshake from a fleet that is up and grants both families.
fn hello() -> crate::control_socket::Hello {
    crate::control_socket::Hello {
        protocol: 1,
        version: "test".to_string(),
        hub_ready: true,
        depth: 0,
        max_depth: 2,
        max_in_flight: 4,
        families: ToolFamilies::default(),
    }
}

/// A worker running exactly `running`.
fn worker(id: &str, running: &[&str]) -> FleetWorker {
    FleetWorker {
        id: id.to_string(),
        address: format!("{id}.local"),
        harness: "claude".to_string(),
        workspace: Some(format!("/work/{id}")),
        running: running.iter().map(|task| task.to_string()).collect(),
        ..FleetWorker::default()
    }
}

/// A store with `run-1` recorded against `sweep` in the given status.
fn store_with_run(
    status: crate::workflows::RunStatus,
) -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    store_with_run_id("run-1", status)
}

/// A store with `run_id` recorded against `sweep` in the given status.
///
/// The run and dispatch registries are process-wide, so a test that registers
/// one must own an id no other test in the binary uses — `cargo test` runs them
/// on one process and concurrently.
fn store_with_run_id(
    run_id: &str,
    status: crate::workflows::RunStatus,
) -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    let mut record = crate::workflows::new_run_record(run_id, "sweep", 1_000);
    record.status = status;
    store.record_run(&record).expect("records the run");
    (root, store)
}

/// A session over `store`, with `fleet` behind it and the family to use it.
///
/// The fleet is attached by hand rather than through
/// [`McpSession::with_fleet`], which would take the families from the stub's
/// handshake and quietly narrow what the test meant to grant.
fn session(store: &Arc<dyn WorkflowStore>, fleet: Arc<dyn FleetBackend>) -> McpSession {
    let mut session = McpSession::local(
        store.clone(),
        crate::workflows::ops::HostPolicy::default(),
        crate::mcp::ToolMode::Full,
    );
    session.families = ToolFamilies::default();
    session.fleet = fleet;
    session
}

#[test]
fn a_dispatch_id_splits_back_into_the_route_and_sequence_that_built_it() {
    let prefix = dispatch_prefix("run-1");
    assert_eq!(prefix, "wf:run-1:");
    assert_eq!(
        split_suffix("wf:run-1:reviewer#3", &prefix),
        Some(("reviewer".to_string(), 3))
    );
    // The default route is spelled out by the engine rather than left empty.
    assert_eq!(
        split_suffix("wf:run-1:default#0", &prefix),
        Some(("default".to_string(), 0))
    );
}

#[test]
fn a_task_id_that_is_not_this_runs_shape_is_dropped_rather_than_guessed_at() {
    let prefix = dispatch_prefix("run-1");
    for foreign in [
        // Another run entirely.
        "wf:run-2:default#0",
        // A fleet dispatch, which carries no route at all.
        "mcp-1f2e3d4c",
        // The shape without a sequence, which a future hub might mint.
        "wf:run-1:default",
        // A sequence that is not a number.
        "wf:run-1:default#last",
    ] {
        assert_eq!(split_suffix(foreign, &prefix), None, "{foreign}");
    }
}

#[test]
fn dispatches_are_attributed_to_their_worker_and_ordered_by_sequence() {
    let workers = vec![
        worker("desktop", &["wf:run-1:reviewer#1", "mcp-unrelated"]),
        worker("laptop", &["wf:run-1:default#0", "wf:run-2:default#0"]),
    ];

    let live = merge(
        dispatches_in(&workers, &dispatch_prefix("run-1")),
        Vec::new(),
    );

    assert_eq!(live.len(), 2, "{live:?}");
    assert_eq!(live[0].sequence, 0);
    assert_eq!(live[0].worker, "laptop");
    assert_eq!(live[0].route, "default");
    assert_eq!(live[1].sequence, 1);
    assert_eq!(live[1].worker, "desktop");
    assert_eq!(live[1].route, "reviewer");
    assert_eq!(live[1].workspace.as_deref(), Some("/work/desktop"));
}

#[tokio::test]
async fn a_running_run_reports_the_harness_sessions_the_fleet_has_in_flight() {
    let (_root, store) = store_with_run(crate::workflows::RunStatus::Running);
    let fleet = Arc::new(StubFleet {
        hello: hello(),
        workers: vec![worker("laptop", &["wf:run-1:default#0"])],
    });

    let answer = detail(&session(&store, fleet), "run-1", StepDetail::Summary)
        .await
        .expect("the run is in the store");

    assert_eq!(answer["run"]["id"], "run-1");
    assert_eq!(answer["live"]["taskIdPrefix"], "wf:run-1:");
    assert_eq!(answer["live"]["harnesses"][0]["worker"], "laptop");
    assert_eq!(answer["live"]["harnesses"][0]["harness"], "claude");
    // The note must not promise more than the hub can see.
    let note = answer["live"]["note"].as_str().unwrap();
    assert!(note.contains("not visible from here"), "{note}");
}

#[tokio::test]
async fn a_run_with_no_fleet_behind_it_still_answers_with_the_record() {
    let (_root, store) = store_with_run(crate::workflows::RunStatus::Running);

    let answer = detail(
        &session(&store, Arc::new(OfflineFleet)),
        "run-1",
        StepDetail::Counts,
    )
    .await
    .expect("the run is in the store");

    // The half that could be answered was answered, and the half that could
    // not says why rather than reading as "nothing is running".
    assert_eq!(answer["run"]["stepDetail"], "counts");
    // The list is still reported, because this process's own dispatches are in
    // it — here there are none, which the note rather than a missing key says.
    assert_eq!(answer["live"]["harnesses"].as_array().unwrap().len(), 0);
    let reason = answer["live"]["fleetUnavailable"].as_str().unwrap();
    assert!(reason.contains("no Medulla fleet is reachable"), "{reason}");
}

#[tokio::test]
async fn a_session_without_the_fleet_family_is_not_quietly_given_the_roster() {
    let (_root, store) = store_with_run(crate::workflows::RunStatus::Running);
    let fleet = Arc::new(StubFleet {
        hello: hello(),
        // A roster the session must not see, even though the backend would
        // serve it: the operator withheld the family.
        workers: vec![worker("laptop", &["wf:run-1:default#0"])],
    });
    let mut session = session(&store, fleet);
    session.families = ToolFamilies::workflows_only();

    let answer = detail(&session, "run-1", StepDetail::Summary)
        .await
        .expect("the run is in the store");

    assert_eq!(answer["live"]["harnesses"].as_array().unwrap().len(), 0);
    let reason = answer["live"]["fleetUnavailable"].as_str().unwrap();
    assert!(reason.contains("not granted the fleet tools"), "{reason}");
}

#[tokio::test]
async fn a_settled_run_says_so_rather_than_reporting_an_absence_of_harnesses() {
    let (_root, store) = store_with_run(crate::workflows::RunStatus::Succeeded);
    let fleet = Arc::new(StubFleet {
        hello: hello(),
        workers: vec![worker("laptop", &[])],
    });

    let answer = detail(&session(&store, fleet), "run-1", StepDetail::Summary)
        .await
        .expect("the run is in the store");

    assert_eq!(answer["live"]["executingHere"], false);
    assert_eq!(answer["live"]["harnesses"].as_array().unwrap().len(), 0);
    let note = answer["live"]["note"].as_str().unwrap();
    assert!(note.contains("has finished"), "{note}");
}

#[tokio::test]
async fn a_run_the_store_has_never_seen_is_an_error_the_caller_can_correct() {
    let (_root, store) = store_with_run(crate::workflows::RunStatus::Succeeded);

    let failure = detail(
        &session(&store, Arc::new(OfflineFleet)),
        "run-ghost",
        StepDetail::Summary,
    )
    .await
    .expect_err("no such run");

    assert!(failure.to_string().contains("run-ghost"), "{failure}");
}

#[test]
fn the_note_distinguishes_a_run_this_process_is_executing_from_one_it_is_not() {
    // Both have no harness in flight, and the difference between them is the
    // whole reason `executingHere` is reported at all.
    assert!(note(Standing::ExecutingHere, 0).contains("between steps"));
    assert!(note(Standing::Unaccounted, 0).contains("another process"));
}

#[test]
fn an_approval_gated_run_is_told_apart_from_one_nobody_can_account_for() {
    // `run_workflow` has already returned by the time a run is parked at a
    // gate, so it is not executing here and has nothing in flight — the same
    // observations as an interrupted run, with the opposite explanation. The
    // durable status is what separates them, so it must be read.
    let parked = json!({ "status": "pending_approval" });
    assert_eq!(Standing::of(&parked, false), Standing::AwaitingApproval);
    let waiting = note(Standing::AwaitingApproval, 0);
    assert!(waiting.contains("approval gate"), "{waiting}");
    assert!(waiting.contains("resumed or rejected"), "{waiting}");
    assert!(!waiting.contains("interrupted"), "{waiting}");
}

#[test]
fn a_runs_standing_is_read_off_the_record_the_caller_is_handed() {
    assert_eq!(
        Standing::of(&json!({ "status": "running" }), true),
        Standing::ExecutingHere
    );
    assert_eq!(
        Standing::of(&json!({ "status": "running" }), false),
        Standing::Unaccounted
    );
    for terminal in ["succeeded", "failed", "cancelled"] {
        assert_eq!(
            Standing::of(&json!({ "status": terminal }), false),
            Standing::Settled,
            "{terminal}"
        );
    }
}

#[test]
fn a_roster_the_control_plane_writes_is_a_roster_this_reader_can_read() {
    // The coupling that makes this module work at all: `worker.list` serializes
    // `FleetWorker` and this deserializes it back. An optional field that is
    // skipped on write but required on read makes the *ordinary* worker — one
    // with no roles, no label, and no workspace — the one that fails.
    let plain = FleetWorker {
        id: "laptop".to_string(),
        address: "laptop.local".to_string(),
        harness: "claude".to_string(),
        ..FleetWorker::default()
    };

    let wire = serde_json::to_value(vec![plain.clone()]).expect("serializes");
    let read: Vec<FleetWorker> = serde_json::from_value(wire).expect("round-trips");

    assert_eq!(read, vec![plain]);
}

#[test]
fn a_dispatch_this_process_made_is_visible_without_any_fleet_roster() {
    // The P1 case: a run started over MCP dispatches onto an embedded host the
    // outer control plane has no row for, so the roster half is empty however
    // busy the run is. Without the local half the tool would report nothing.
    let run = "run-embedded";
    let prefix = dispatch_prefix(run);
    let _guard = run::dispatches::record(
        run,
        InFlightDispatch {
            task_id: format!("{prefix}reviewer#2"),
            worker: "local".to_string(),
            harness: "codex".to_string(),
            workspace: Some("/srv/app".to_string()),
        },
    );

    let live = dispatches_here(run, &prefix);

    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0].route, "reviewer");
    assert_eq!(live[0].sequence, 2);
    assert_eq!(live[0].worker, "local");
    assert_eq!(live[0].harness, "codex");
    assert_eq!(live[0].workspace.as_deref(), Some("/srv/app"));
}

#[test]
fn a_task_id_the_local_registry_holds_under_another_shape_is_dropped() {
    // Same guard as the roster half: an id that does not parse is left out
    // rather than reported with an invented route.
    let run = "run-odd-id";
    let _guard = run::dispatches::record(
        run,
        InFlightDispatch {
            task_id: "not-a-workflow-task".to_string(),
            worker: "local".to_string(),
            harness: "claude".to_string(),
            workspace: None,
        },
    );

    assert!(dispatches_here(run, &dispatch_prefix(run)).is_empty());
}

#[test]
fn the_two_halves_merge_without_double_counting_one_dispatch() {
    let run = "run-merge";
    let prefix = dispatch_prefix(run);
    let fleet = dispatches_in(&[worker("laptop", &["wf:run-merge:default#0"])], &prefix);
    // The same task id the roster already accounted for, plus one only this
    // process knows about.
    let _shared = run::dispatches::record(
        run,
        InFlightDispatch {
            task_id: "wf:run-merge:default#0".to_string(),
            worker: "default-worker-address".to_string(),
            harness: String::new(),
            workspace: None,
        },
    );
    let _only_here = run::dispatches::record(
        run,
        InFlightDispatch {
            task_id: "wf:run-merge:builder#1".to_string(),
            worker: "local".to_string(),
            harness: "claude".to_string(),
            workspace: None,
        },
    );

    let merged = merge(fleet, dispatches_here(run, &prefix));

    assert_eq!(merged.len(), 2, "{merged:?}");
    // The roster's row won for the shared id: it names the machine that took
    // the work rather than the address it was routed to.
    assert_eq!(merged[0].sequence, 0);
    assert_eq!(merged[0].worker, "laptop");
    assert_eq!(merged[1].sequence, 1);
    assert_eq!(merged[1].worker, "local");
}

#[tokio::test]
async fn a_run_executing_here_reports_its_own_harness_session_and_says_it_is_live() {
    let run = "run-executing-here";
    let (_root, store) = store_with_run_id(run, crate::workflows::RunStatus::Running);
    // Registered so the run reads as executing in this process, which is what
    // a run started over this same MCP server looks like.
    let (_run_guard, _signal) = run::RunGuard::register(run);
    let _dispatch = run::dispatches::record(
        run,
        InFlightDispatch {
            task_id: "wf:run-executing-here:default#0".to_string(),
            worker: "local".to_string(),
            harness: "claude".to_string(),
            workspace: Some("/srv/app".to_string()),
        },
    );

    // An offline fleet on purpose: the embedded host is not in any roster, so
    // this is exactly the shape the roster-only join answered nothing for.
    let answer = detail(
        &session(&store, Arc::new(OfflineFleet)),
        run,
        StepDetail::Summary,
    )
    .await
    .expect("the run is in the store");

    assert_eq!(answer["live"]["executingHere"], true);
    let harnesses = answer["live"]["harnesses"].as_array().unwrap();
    assert_eq!(harnesses.len(), 1, "{harnesses:?}");
    assert_eq!(harnesses[0]["worker"], "local");
    assert_eq!(harnesses[0]["route"], "default");
    let note = answer["live"]["note"].as_str().unwrap();
    assert!(note.contains("running now"), "{note}");
}

#[tokio::test]
async fn an_approval_gated_run_reads_as_waiting_rather_than_as_lost() {
    let run = "run-parked";
    let (_root, store) = store_with_run_id(run, crate::workflows::RunStatus::PendingApproval);
    let fleet = Arc::new(StubFleet {
        hello: hello(),
        workers: vec![worker("laptop", &[])],
    });

    let answer = detail(&session(&store, fleet), run, StepDetail::Summary)
        .await
        .expect("the run is in the store");

    assert_eq!(answer["live"]["executingHere"], false);
    let note = answer["live"]["note"].as_str().unwrap();
    assert!(note.contains("approval gate"), "{note}");
}

#[tokio::test]
async fn a_fleet_that_refuses_the_roster_says_what_it_said() {
    struct RefusingFleet;

    #[async_trait::async_trait]
    impl FleetBackend for RefusingFleet {
        fn hello(&self) -> Option<&crate::control_socket::Hello> {
            None
        }

        async fn call(&self, _op: &str, _params: Value) -> Result<Value, ControlError> {
            Err(ControlError::Transport(
                "the control socket closed mid-call".to_string(),
            ))
        }
    }

    let (_root, store) = store_with_run(crate::workflows::RunStatus::Running);

    let answer = detail(
        &session(&store, Arc::new(RefusingFleet)),
        "run-1",
        StepDetail::Summary,
    )
    .await
    .expect("the run is in the store");

    let reason = answer["live"]["fleetUnavailable"].as_str().unwrap();
    assert!(reason.contains("closed mid-call"), "{reason}");
}
