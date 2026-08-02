//! Run-record listing: newest first, scoped to their workflow, and a run that
//! was never recorded is a distinct error rather than a silent `None`.

use super::*;

#[test]
fn runs_are_listed_newest_first_and_scoped_to_their_workflow() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());

    store
        .record_run(&new_run_record("r1", "alpha", 100))
        .unwrap();
    store
        .record_run(&new_run_record("r2", "alpha", 300))
        .unwrap();
    store
        .record_run(&new_run_record("r3", "beta", 200))
        .unwrap();

    let alpha = store.list_runs("alpha").unwrap();
    let ids: Vec<&str> = alpha.iter().map(|r| r.id.as_str()).collect();

    assert_eq!(ids, vec!["r2", "r1"], "newest first");
    assert_eq!(store.list_runs("beta").unwrap().len(), 1);
    assert_eq!(store.list_runs("unknown").unwrap().len(), 0);
}

#[test]
fn a_run_record_survives_being_rewritten_as_it_settles() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut run = new_run_record("r1", "alpha", 100);
    store.record_run(&run).unwrap();

    run.status = RunStatus::PendingApproval;
    run.pending_approvals = vec!["review".into()];
    store.record_run(&run).unwrap();

    let loaded = require_run(&store, "r1").expect("found");
    assert_eq!(loaded.status, RunStatus::PendingApproval);
    assert_eq!(loaded.pending_approvals, vec!["review".to_string()]);
    assert!(!loaded.status.is_settled(), "an approval gate is resumable");
}

#[test]
fn asking_for_a_run_that_was_never_recorded_is_an_error_not_a_silent_none() {
    let root = tempfile::tempdir().unwrap();
    let err = require_run(&store_in(root.path()), "ghost").expect_err("no such run");
    assert!(matches!(err, WorkflowError::RunNotFound(_)), "got {err:?}");
}
