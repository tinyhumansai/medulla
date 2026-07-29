//! Tests for `CopilotSession::create`.
//!
//! Split out of [`super::tests`] once the create-turn cases pushed it over the
//! repository's 500-line file ceiling. Reuses that module's fixtures via the
//! shared [`super::support`] module (`store`, `session`, `document`,
//! `StubHarness`, `BrokenHarness`, `UnreadableStore`).

use std::sync::Arc;

use super::support::*;
use super::*;

#[tokio::test]
async fn a_create_turn_reports_the_workflow_that_appeared() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "built it");
    // Standing in for the `workflow_create` call a real copilot would make.
    harness.edit = Some(Box::new(|store| {
        store.save(&document("fresh")).expect("save");
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .create("summarise new issues daily", None)
        .await
        .expect("create");

    assert_eq!(outcome.created.as_deref(), Some("fresh"));
    assert_eq!(outcome.changes, vec!["+ workflow fresh".to_string()]);
    assert_eq!(outcome.reply, "built it");
}

#[tokio::test]
async fn a_create_turn_is_told_to_create_rather_than_to_edit() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    session.create("something new", None).await.expect("create");

    let seen = harness.seen.lock().unwrap();
    let prompt = &seen[0].instruction;
    assert!(prompt.contains("workflow_create"), "{prompt}");
    assert!(prompt.contains("something new"), "{prompt}");
    // The existing workflow must not be named: this turn only adds, and naming
    // one is how an agent talks itself into editing it.
    assert!(!prompt.contains("sweep"), "{prompt}");
}

#[tokio::test]
async fn a_create_turn_that_built_nothing_is_not_a_failure() {
    let (_root, store) = store();
    // No edit: the agent answered a question instead of building.
    let harness = Arc::new(StubHarness::new(store.clone(), "you could try…"));
    let session = session(store, harness);

    let outcome = session
        .create("what could I build?", None)
        .await
        .expect("create");

    assert_eq!(outcome.created, None);
    assert!(outcome.changes.is_empty());
    assert_eq!(outcome.reply, "you could try…");
}

#[tokio::test]
async fn a_create_turn_ignores_a_workflow_that_was_already_there() {
    let (_root, store) = store();
    // The stub rewrites the *existing* workflow rather than adding one.
    let mut harness = StubHarness::new(store.clone(), "touched it");
    harness.edit = Some(Box::new(|store| {
        let mut record = document("sweep");
        record.name = "Renamed".into();
        store.save(&record).expect("save");
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .create("make something", None)
        .await
        .expect("create");

    // Nothing is new, so nothing is reported as created — the comparison is by
    // id against the catalogue, not "did the store change at all".
    assert_eq!(outcome.created, None);
    assert!(outcome.changes.is_empty());
}

#[tokio::test]
async fn a_create_turn_reports_every_workflow_it_made_not_only_the_first() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "built them");
    harness.edit = Some(Box::new(|store| {
        store.save(&document("alpha")).expect("save");
        store.save(&document("beta")).expect("save");
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session.create("build two", None).await.expect("create");

    // Both reported — an unmentioned second workflow is one the operator has
    // and cannot see. The first by id is offered as the one to select.
    assert_eq!(
        outcome.changes,
        vec![
            "+ workflow alpha".to_string(),
            "+ workflow beta".to_string()
        ]
    );
    assert_eq!(outcome.created.as_deref(), Some("alpha"));
}

#[tokio::test]
async fn a_create_turn_fails_rather_than_guessing_when_the_catalogue_cannot_be_read() {
    let store: Arc<dyn WorkflowStore> = Arc::new(UnreadableStore);
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    let err = session
        .create("build me one", None)
        .await
        .expect_err("fails");

    assert!(matches!(err, WorkflowError::Io { .. }), "{err}");
    assert!(
        harness.seen.lock().unwrap().is_empty(),
        "an unreadable catalogue must stop the turn before it dispatches: a \
         silent empty set makes every existing workflow look newly created"
    );
}

#[tokio::test]
async fn a_create_turn_that_cannot_reach_a_harness_fails() {
    let (_root, store) = store();
    let session = session(store, Arc::new(BrokenHarness));

    let err = session.create("build me one", None).await.unwrap_err();

    assert!(err.to_string().contains("no harness installed"));
}
