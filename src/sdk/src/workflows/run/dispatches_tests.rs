//! Tests for the in-flight harness dispatch registry.
//!
//! The registry is process-wide, so each case uses a run id of its own rather
//! than clearing shared state — tests in a crate run concurrently, and a reset
//! between them would race.

use super::*;

fn dispatch(task_id: &str) -> InFlightDispatch {
    InFlightDispatch {
        task_id: task_id.to_string(),
        worker: "local".to_string(),
        harness: "claude".to_string(),
        workspace: Some("/tmp/work".to_string()),
    }
}

#[test]
fn a_recorded_dispatch_is_visible_until_its_guard_drops() {
    let run = "run-visible";
    assert!(in_flight(run).is_empty());
    {
        let _guard = record(run, dispatch("wf:run-visible:default#0"));
        let live = in_flight(run);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].task_id, "wf:run-visible:default#0");
        assert_eq!(live[0].harness, "claude");
    }
    assert!(in_flight(run).is_empty());
}

#[test]
fn concurrent_dispatches_withdraw_independently() {
    let run = "run-concurrent";
    let first = record(run, dispatch("wf:run-concurrent:default#0"));
    let _second = record(run, dispatch("wf:run-concurrent:reviewer#1"));
    assert_eq!(in_flight(run).len(), 2);
    drop(first);
    let live = in_flight(run);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].task_id, "wf:run-concurrent:reviewer#1");
}

#[test]
fn a_run_with_nothing_in_flight_reports_nothing() {
    assert!(in_flight("run-never-dispatched").is_empty());
}

#[test]
fn a_dispatch_carries_the_worker_and_workspace_it_went_to() {
    let _guard = record("run-attributed", dispatch("wf:run-attributed:default#0"));
    let live = in_flight("run-attributed");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].worker, "local");
    assert_eq!(live[0].workspace.as_deref(), Some("/tmp/work"));
}

#[test]
fn one_run_does_not_see_another_run_s_dispatches() {
    let _mine = record("run-mine", dispatch("wf:run-mine:default#0"));
    let _theirs = record("run-theirs", dispatch("wf:run-theirs:default#0"));
    assert_eq!(in_flight("run-mine").len(), 1);
    assert_eq!(in_flight("run-mine")[0].task_id, "wf:run-mine:default#0");
}
