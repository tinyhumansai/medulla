//! Failure-note deduplication and observation-budget tests.

use super::*;

#[tokio::test]
async fn a_failure_already_recorded_is_not_recorded_again() {
    let id = "a-failure-already-recorded-is-not-again";
    let (_home, store, run) = fixture(id);

    record_failure_note(&store, &run).expect("the first write lands");
    let outcome = session(store.clone(), silent(&store, "ok"))
        .evolve(id, EvolveTrigger::Failure(run.id.clone()), None)
        .await
        .expect("the pass completes");

    assert!(outcome.notes.is_empty());
    assert_eq!(store.list_notes(id).expect("readable").len(), 1);
}

#[tokio::test]
async fn a_different_failure_of_the_same_workflow_is_still_recorded() {
    let id = "a-different-failure-is-still-recorded";
    let (_home, store, run) = fixture(id);
    record_failure_note(&store, &run).expect("the first write lands");

    let mut second = new_run_record("run-2", id, 9);
    second.status = RunStatus::Failed;
    second.error = Some("a different failure".into());
    store.record_run(&second).expect("records");
    record_failure_note(&store, &second).expect("the second write lands");

    assert_eq!(store.list_notes(id).expect("readable").len(), 2);
}

#[test]
fn the_brief_keeps_rejections_and_constraints_when_observations_crowd_them_out() {
    let mut notes = Vec::new();
    for (kind, source, text) in [
        (
            NoteKind::Rejection,
            NoteSource::Operator,
            "do not delete the step",
        ),
        (
            NoteKind::Constraint,
            NoteSource::Operator,
            "tests run before deploy",
        ),
    ] {
        notes.push(WorkflowNote {
            id: mint_note_id(1),
            workflow_id: "sweep".into(),
            kind,
            text: text.into(),
            recorded_at: 1,
            source,
            run_ids: Vec::new(),
            superseded_by: None,
            pinned: true,
        });
    }
    for index in 0..50u64 {
        notes.push(WorkflowNote {
            id: mint_note_id(100 + index),
            workflow_id: "sweep".into(),
            kind: NoteKind::Observation,
            text: format!("run {index} failed"),
            recorded_at: 100 + index,
            source: NoteSource::System,
            run_ids: Vec::new(),
            superseded_by: None,
            pinned: false,
        });
    }
    notes.sort_by(|a, b| b.id.cmp(&a.id));

    let brief = super::super::session::select_for_brief(notes, 10);

    assert_eq!(brief.len(), 10);
    assert!(brief.iter().any(|n| n.text == "do not delete the step"));
    assert!(brief.iter().any(|n| n.text == "tests run before deploy"));
    assert!(brief.iter().any(|n| n.text == "run 49 failed"));
    let mut sorted = brief.clone();
    sorted.sort_by(|a, b| b.id.cmp(&a.id));
    assert_eq!(brief, sorted);
}

#[test]
fn a_brief_within_budget_keeps_everything_in_order() {
    let notes: Vec<WorkflowNote> = (0..3u64)
        .map(|index| WorkflowNote {
            id: mint_note_id(index),
            workflow_id: "sweep".into(),
            kind: NoteKind::Observation,
            text: format!("note {index}"),
            recorded_at: index,
            source: NoteSource::System,
            run_ids: Vec::new(),
            superseded_by: None,
            pinned: false,
        })
        .collect();

    assert_eq!(
        super::super::session::select_for_brief(notes.clone(), 40),
        notes
    );
}
