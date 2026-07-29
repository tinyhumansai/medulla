//! Note supersession, pinning, and successful-run behavior.

#![cfg(feature = "workflows")]

use super::*;

#[tokio::test]
async fn a_note_can_replace_an_earlier_one_it_disproves() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");

    let guess = ops::add_note(
        &store,
        "sweep",
        "hypothesis",
        "the step is probably just slow",
        Vec::new(),
        NoteSource::Agent { model: None },
        Vec::new(),
    )
    .expect("the note records");
    let guess_id = guess["recorded"].as_str().expect("an id").to_string();

    let settled = ops::add_note(
        &store,
        "sweep",
        "observation",
        "the host refuses code nodes; speed was never the issue",
        Vec::new(),
        NoteSource::Agent { model: None },
        vec![guess_id.clone()],
    )
    .expect("the note records");

    assert_eq!(settled["superseded"], json!([guess_id]));
    assert_eq!(store.list_notes("sweep").expect("readable").len(), 2);
    let current = medulla::workflows::current_notes(store.as_ref(), "sweep").expect("readable");
    assert_eq!(current.len(), 1);
    assert!(current[0].text.contains("refuses code nodes"));
}

#[tokio::test]
async fn a_note_cannot_supersede_itself() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");

    let recorded = ops::add_note(
        &store,
        "sweep",
        "observation",
        "something",
        Vec::new(),
        NoteSource::Operator,
        vec!["self".into()],
    )
    .expect("the note records");
    let id = recorded["recorded"].as_str().expect("an id").to_string();

    let echoed = ops::add_note(
        &store,
        "sweep",
        "observation",
        "another",
        Vec::new(),
        NoteSource::Operator,
        vec![id.clone(), id],
    )
    .expect("the note records");
    let echoed_id = echoed["recorded"].as_str().expect("an id");

    let current = medulla::workflows::current_notes(store.as_ref(), "sweep").expect("readable");
    assert!(current.iter().any(|note| note.id == echoed_id));
}

#[tokio::test]
async fn an_operator_note_is_pinned_and_an_agent_note_is_not() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");

    ops::add_note(
        &store,
        "sweep",
        "constraint",
        "never remove the work step",
        Vec::new(),
        NoteSource::Operator,
        Vec::new(),
    )
    .expect("records");
    ops::add_note(
        &store,
        "sweep",
        "hypothesis",
        "maybe it is the timeout",
        Vec::new(),
        NoteSource::Agent { model: None },
        Vec::new(),
    )
    .expect("records");

    let notes = store.list_notes("sweep").expect("readable");
    let operator = notes
        .iter()
        .find(|n| n.source == NoteSource::Operator)
        .expect("the operator's note");
    let agent = notes
        .iter()
        .find(|n| matches!(n.source, NoteSource::Agent { .. }))
        .expect("the agent's note");
    assert!(operator.pinned);
    assert!(!agent.pinned);
}

#[tokio::test]
async fn a_run_that_succeeded_is_never_recorded_as_a_failure() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    let document = json!({
        "id": "fine",
        "name": "Fine",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "transform", "name": "Work",
              "config": { "set": { "ok": true } } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string();
    ops::create(&store, &document, "fine").expect("installs");
    run_once(&store, home.path(), "fine").await;
    let run = store.list_runs("fine").expect("readable").remove(0);

    let recorded = medulla::workflows::evolve::record_failure_note(&store, &run)
        .expect("not an error, just nothing to say");

    assert!(recorded.is_none());
    assert!(store.list_notes("fine").expect("readable").is_empty());
}
