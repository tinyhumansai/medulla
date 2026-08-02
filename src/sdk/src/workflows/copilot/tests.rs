//! Tests for one copilot turn.
//!
//! The store is real (a temporary directory) and the diff is real; only the
//! harness is a stand-in, because the alternative is starting a coding agent.
//! The stand-in is where the *edits* come from too: a real copilot edits through
//! the MCP tools, and a stub that writes to the same store is indistinguishable
//! from that as far as this module is concerned.
//!
//! Fixtures (`store`, `session`, `document`, `StubHarness`, and friends) live
//! in the sibling [`super::support`] module, shared with [`super::create_tests`]
//! — that split happened once the create-turn cases pushed this file over the
//! 500-line ceiling.

use std::sync::Arc;

use serde_json::json;

use super::support::*;
use super::*;
use crate::hub::RunError;

#[tokio::test]
async fn a_turn_dispatches_the_operator_instruction_inside_a_scoped_prompt() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "looked at it"));
    let session = session(store, harness.clone());

    session
        .turn("sweep", "what does this do?", None)
        .await
        .expect("turn");

    let seen = harness.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].instruction.contains("what does this do?"));
    assert!(seen[0].instruction.contains("id: sweep"));
    assert_eq!(seen[0].worker_address, "worker");
    assert!(
        seen[0].workflow.is_none(),
        "an authoring turn must never run the graph it is editing"
    );
}

#[tokio::test]
async fn each_turn_gets_its_own_task_id_so_two_are_never_deduped() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    session.turn("sweep", "one", None).await.expect("first");
    session.turn("sweep", "two", None).await.expect("second");

    let seen = harness.seen.lock().unwrap();
    assert_ne!(seen[0].task_id, seen[1].task_id);
    assert_eq!(
        seen[0].abort_id, seen[0].task_id,
        "the turn is abortable by the id it was dispatched under"
    );
}

#[tokio::test]
async fn every_turn_in_one_pane_names_the_same_conversation() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    session
        .turn("sweep", "add a step", None)
        .await
        .expect("one");
    session
        .turn("sweep", "now do the same to the other node", None)
        .await
        .expect("two");

    let seen = harness.seen.lock().unwrap();
    // Both turns are one conversation, which is what makes the second
    // instruction intelligible — it names no node, and only the first turn's
    // context says which one "the other" is.
    assert_eq!(seen[0].conversation.as_deref(), Some("pane-1"));
    assert_eq!(seen[1].conversation.as_deref(), Some("pane-1"));
}

#[tokio::test]
async fn a_create_turn_shares_the_panes_conversation_too() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    session.create("build me one", None).await.expect("create");

    // The follow-up to "build me one" is almost always "now change it", and
    // that has to reach the session that built it.
    assert_eq!(
        harness.seen.lock().unwrap()[0].conversation.as_deref(),
        Some("pane-1")
    );
}

#[tokio::test]
async fn a_repair_turn_hands_the_agent_the_failure_rather_than_making_it_hunt() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "the worker was offline"));
    let session = session(store, harness.clone());

    session
        .repair(
            "sweep",
            "why did this fail last night?",
            crate::workflows::FailedRun {
                id: "run-42".into(),
                error: Some("worker refused the task".into()),
                failing_nodes: vec!["work".into()],
            },
            None,
        )
        .await
        .expect("repair");

    let seen = harness.seen.lock().unwrap();
    let prompt = &seen[0].instruction;
    assert!(prompt.contains("run-42"), "{prompt}");
    assert!(prompt.contains("worker refused the task"), "{prompt}");
    assert!(prompt.contains("work"), "{prompt}");
    // Still an authoring turn: diagnosing a failed run must not re-run it.
    assert!(seen[0].workflow.is_none());
}

#[tokio::test]
async fn a_turn_that_changes_nothing_reports_no_changes() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "it sweeps the repo"));
    let session = session(store, harness);

    let outcome = session
        .turn("sweep", "explain it", None)
        .await
        .expect("turn");

    assert_eq!(outcome.reply, "it sweeps the repo");
    assert!(outcome.changes.is_empty());
}

#[tokio::test]
async fn a_turn_reports_what_the_store_actually_holds_afterwards() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "added the step");
    harness.edit = Some(Box::new(|store| {
        let mut record = store.get("sweep").unwrap().unwrap();
        record.graph.nodes.push(
            serde_json::from_value(json!({
                "id": "notify", "kind": "tool_call", "name": "Notify",
                "config": { "tool": "slack" },
            }))
            .unwrap(),
        );
        record.graph.edges.push(
            serde_json::from_value(json!({ "from_node": "work", "to_node": "notify" })).unwrap(),
        );
        store.save(&record).unwrap();
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .turn("sweep", "notify slack at the end", None)
        .await
        .expect("turn");

    assert!(
        outcome
            .changes
            .contains(&"+ node notify (tool_call)".to_string()),
        "{:?}",
        outcome.changes
    );
    assert!(
        outcome
            .changes
            .contains(&"+ edge work → notify".to_string()),
        "{:?}",
        outcome.changes
    );
}

#[tokio::test]
async fn an_agent_that_claims_an_edit_it_did_not_make_is_not_believed() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "I added a Slack step!"));
    let session = session(store, harness);

    let outcome = session
        .turn("sweep", "add slack", None)
        .await
        .expect("turn");

    assert!(
        outcome.changes.is_empty(),
        "the store is the only witness: {:?}",
        outcome.changes
    );
}

#[tokio::test]
async fn progress_frames_reach_the_caller_as_they_arrive() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "done");
    harness.statuses = vec!["reading the graph".into(), "applying ops".into()];
    let session = session(store, Arc::new(harness));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    session.turn("sweep", "go", Some(tx)).await.expect("turn");

    let mut seen = Vec::new();
    while let Ok(line) = rx.try_recv() {
        seen.push(line);
    }
    assert_eq!(seen, vec!["reading the graph", "applying ops"]);
}

#[tokio::test]
async fn a_turn_against_an_unknown_workflow_fails_before_dispatching_anything() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    let err = session
        .turn("absent", "go", None)
        .await
        .expect_err("no such workflow");

    assert!(matches!(err, WorkflowError::NotFound(_)), "{err}");
    assert!(
        harness.seen.lock().unwrap().is_empty(),
        "nothing should have been dispatched"
    );
}

#[tokio::test]
async fn a_failed_dispatch_surfaces_the_harness_error() {
    let (_root, store) = store();
    let session = session(store, Arc::new(BrokenHarness));

    let err = session.turn("sweep", "go", None).await.expect_err("broken");

    assert!(err.to_string().contains("no harness installed"), "{err}");
}

#[tokio::test]
async fn a_workflow_the_turn_deleted_is_reported_as_a_removal() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "removed it");
    harness.edit = Some(Box::new(|store| {
        store.delete("sweep").unwrap();
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .turn("sweep", "delete it", None)
        .await
        .expect("turn");

    assert_eq!(outcome.reply, "removed it");
    // Not empty: the caller only refreshes its catalogue when something
    // changed, so a silent deletion leaves the workflow on screen.
    assert_eq!(outcome.changes, vec!["− workflow sweep".to_string()]);
}

#[tokio::test]
async fn a_turn_that_only_edited_the_description_still_reports_a_change() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "reworded it");
    harness.edit = Some(Box::new(|store| {
        let mut record = store.get("sweep").unwrap().unwrap();
        record.description = "sweeps the repo every night".into();
        store.save(&record).unwrap();
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .turn("sweep", "describe it better", None)
        .await
        .expect("turn");

    assert_eq!(outcome.changes, vec!["~ description".to_string()]);
}

#[tokio::test]
async fn a_turn_that_disabled_the_workflow_reports_it() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "turned it off");
    harness.edit = Some(Box::new(|store| {
        let mut record = store.get("sweep").unwrap().unwrap();
        record.enabled = false;
        store.save(&record).unwrap();
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .turn("sweep", "stop it running", None)
        .await
        .expect("turn");

    assert_eq!(outcome.changes, vec!["~ disabled".to_string()]);
}

#[tokio::test]
async fn dispatch_failures_keep_the_shape_the_hub_reported() {
    let (_root, store) = store();

    for (run_error, expected) in [
        (RunError::Timeout, "did not respond in time"),
        (RunError::Aborted, "aborted"),
        (
            RunError::Busy("worker at capacity".into()),
            "worker at capacity",
        ),
        (RunError::Transport("relay is down".into()), "relay is down"),
    ] {
        let session = session(store.clone(), Arc::new(FailingHarness(run_error)));
        let err = session.turn("sweep", "go", None).await.expect_err("fails");
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[tokio::test]
async fn a_reply_is_trimmed_so_it_does_not_open_the_pane_with_blank_lines() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "\n\n  done  \n"));
    let session = session(store, harness);

    let outcome = session.turn("sweep", "go", None).await.expect("turn");

    assert_eq!(outcome.reply, "done");
}
