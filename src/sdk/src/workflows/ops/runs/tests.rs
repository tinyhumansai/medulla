//! Tests for the run-reading operations and their step projections.
//!
//! What these are about is size. A run record is the right answer to "why did
//! this fail" and the wrong answer to "did it finish", and the projections are
//! how one surface serves both — so the tests check that the cheap levels stay
//! cheap and that nothing but step detail changes between them.

use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::workflows::{new_run_record, FileWorkflowStore, RunStatus, RunStep};

/// A store on a scratch root, with its temp directory kept alive.
fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().expect("a temp dir");
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    (root, store)
}

/// A settled run whose one step emitted `output_bytes` of text.
fn recorded(store: &Arc<dyn WorkflowStore>, run_id: &str, output_bytes: usize) {
    let mut record = new_run_record(run_id, "sweep", 1_000);
    record.status = RunStatus::Succeeded;
    record.finished_at = Some(2_000);
    record.steps = vec![RunStep {
        node_id: "work".to_string(),
        status: "ok".to_string(),
        duration_ms: 900,
        input: Some(json!("a very long prompt")),
        output: Some(json!("o".repeat(output_bytes))),
        diagnostics: vec!["$.missing resolved to null".to_string()],
        transcript: Vec::new(),
    }];
    store.record_run(&record).expect("records the run");
}

#[test]
fn a_summary_keeps_what_happened_and_drops_the_prompt() {
    let (_root, store) = store();
    recorded(&store, "run-1", 16);

    let run = get_run(&store, "run-1", StepDetail::Summary).unwrap();

    let step = &run["steps"][0];
    assert_eq!(step["nodeId"], "work");
    assert_eq!(step["status"], "ok");
    assert_eq!(step["durationMs"], 900);
    // Small enough to survive whole: a summary bounds an output, it does not
    // truncate one that already fits.
    assert_eq!(step["output"], json!("o".repeat(16)));
    assert_eq!(step["diagnostics"][0], "$.missing resolved to null");
    // The prompt is the largest field in a run record and says nothing about
    // what the run did with it.
    assert!(step.get("input").is_none(), "{step}");
}

#[test]
fn a_summary_bounds_an_output_that_would_swamp_the_reply() {
    let (_root, store) = store();
    recorded(&store, "run-1", 200_000);

    let run = get_run(&store, "run-1", StepDetail::Summary).unwrap();

    let output = &run["steps"][0]["output"];
    assert!(tinyflows::store::is_truncated(output));
    assert!(output["preview"].as_str().unwrap().starts_with("\"oo"));
    // The point of the level: whatever the step emitted, the projection is
    // small enough that a caller can hold a hundred of them.
    let projected = serde_json::to_string(&run).unwrap().len();
    assert!(projected < 8_000, "{projected} bytes is not a summary");
}

#[test]
fn full_detail_is_the_record_itself_plus_the_count() {
    let (_root, store) = store();
    recorded(&store, "run-1", 32);

    let run = get_run(&store, "run-1", StepDetail::Full).unwrap();

    assert_eq!(run["steps"][0]["input"], json!("a very long prompt"));
    assert_eq!(run["steps"][0]["output"], json!("o".repeat(32)));
    assert_eq!(run["stepCount"], 1);
    assert_eq!(run["stepDetail"], "full");
    // Nothing is elided, so nothing tells the caller where to find more.
    assert!(run.get("stepsElidedHint").is_none(), "{run}");
}

#[test]
fn a_history_listing_carries_no_step_bodies_at_all() {
    let (_root, store) = store();
    recorded(&store, "run-1", 200_000);
    recorded(&store, "run-2", 200_000);

    let runs = list_runs(&store, "sweep", StepDetail::Counts).unwrap();

    let listed = runs["runs"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    for run in listed {
        // Status, timestamps and step count — which run to look at, and how it
        // ended — without a byte of what any step emitted.
        assert!(run.get("steps").is_none(), "{run}");
        assert_eq!(run["stepCount"], 1);
        assert_eq!(run["status"], "succeeded");
        assert_eq!(run["startedAt"], 1_000);
        assert!(run["stepsElidedHint"]
            .as_str()
            .unwrap()
            .contains("workflow_run_get"));
    }
    let listing = serde_json::to_string(&runs).unwrap().len();
    assert!(listing < 2_000, "{listing} bytes for two runs");
}

#[test]
fn every_level_agrees_about_everything_except_the_steps() {
    let (_root, store) = store();
    recorded(&store, "run-1", 64);

    let levels = [StepDetail::Full, StepDetail::Summary, StepDetail::Counts]
        .map(|detail| get_run(&store, "run-1", detail).unwrap())
        .map(|mut run| {
            let object = run.as_object_mut().unwrap();
            object.remove("steps");
            object.remove("stepDetail");
            object.remove("stepsElidedHint");
            run
        });

    assert_eq!(levels[0], levels[1]);
    assert_eq!(levels[1], levels[2]);
}

#[test]
fn an_unknown_step_level_says_which_ones_exist() {
    let err = StepDetail::parse(Some("brief"), StepDetail::Summary).unwrap_err();

    let message = err.to_string();
    for level in ["full", "summary", "counts"] {
        assert!(message.contains(level), "{message}");
    }
}

#[test]
fn an_absent_or_empty_step_level_is_the_default_rather_than_an_error() {
    assert_eq!(
        StepDetail::parse(None, StepDetail::Counts).unwrap(),
        StepDetail::Counts
    );
    assert_eq!(
        StepDetail::parse(Some("  "), StepDetail::Summary).unwrap(),
        StepDetail::Summary
    );
    // Spelled either way, because "no steps" is the more obvious name for it.
    assert_eq!(
        StepDetail::parse(Some("none"), StepDetail::Full).unwrap(),
        StepDetail::Counts
    );
}

#[test]
fn a_run_that_is_still_going_reads_back_before_it_settles() {
    let (_root, store) = store();
    // What an async `workflow_run` leaves behind for its caller to poll: the
    // record exists, and says the run has not finished, from the moment the run
    // is admitted.
    store
        .record_run(&new_run_record("run-1", "sweep", 1_000))
        .expect("records the run");

    let run = get_run(&store, "run-1", StepDetail::Summary).unwrap();

    assert_eq!(run["status"], "running");
    assert_eq!(run["stepCount"], 0);
    assert_eq!(run["steps"], json!([]));
    assert!(run.get("finishedAt").is_none(), "{run}");
}

#[test]
fn waiting_modes_say_whether_they_hold_the_call_open() {
    assert!(!Wait::No.blocks());
    assert!(Wait::Forever.blocks());
    assert!(Wait::Until(Duration::from_millis(1)).blocks());
}

#[test]
fn an_admitted_run_answers_with_its_id_and_how_to_follow_it() {
    let answer: Value = admitted("run-1", "sweep", Some(Duration::from_secs(30)));

    assert_eq!(answer["runId"], "run-1");
    assert_eq!(answer["workflowId"], "sweep");
    assert_eq!(answer["status"], "running");
    assert_eq!(answer["waitedMs"], 30_000);
    // The next call the caller should make, named in the answer — the failure
    // this shape exists to fix was a caller left with no run id at all.
    assert!(answer["note"]
        .as_str()
        .unwrap()
        .contains("workflow_run_get"));

    // With no budget there was no wait to report, and saying "waited 0ms"
    // would read as a wait that failed rather than one never asked for.
    let never = admitted("run-1", "sweep", None);
    assert!(never.get("waitedMs").is_none(), "{never}");
}
