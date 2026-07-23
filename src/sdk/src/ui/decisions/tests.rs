//! Decision-fold tests for ordering, dedupe, and answered-item removal.

use crate::harness_contract::{
    HarnessState, HarnessStatus, HarnessUsage, TrackedTask, TrackedTaskStatus,
};
use crate::ui::agents::{AgentLane, AgentRole, TaskState, TaskStatus};

use super::{decision_items, DecisionKind};

fn status() -> HarnessStatus {
    HarnessStatus {
        state: HarnessState::Running,
        queued: 0,
        active_instruction_id: None,
        active_cycle_id: Some("cycle-1".into()),
        tasks: vec![TrackedTask {
            id: "task-1".into(),
            title: "Choose schema".into(),
            detail: Some("Do not change the public API".into()),
            status: TrackedTaskStatus::Blocked,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:01Z".into(),
            instruction_id: None,
            delegated_task_ids: vec!["cycle-1/t:task-1".into()],
            notes: vec![],
        }],
        running_delegations: 1,
        usage: HarnessUsage::default(),
        last_result: None,
        escalations: vec![
            "Needs release approval".into(),
            "Needs release approval".into(),
            " ".into(),
        ],
    }
}

fn lane(question: bool) -> AgentLane {
    AgentLane {
        key: "agent:dev".into(),
        label: "dev".into(),
        role: AgentRole::Worker,
        turns: vec![],
        last_at: 0,
        tasks: vec![TaskState {
            task_id: "cycle-1/t:task-1".into(),
            status: TaskStatus::Running,
            turns: 0,
            last_at: 0,
            turn_blocks: vec![],
            attention: question.then(|| "confirm: use v2?".into()),
            question_id: question.then(|| "q1".into()),
        }],
        context_tokens: None,
        harness_label: None,
        agent_id: Some("dev".into()),
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 1,
    }
}

#[test]
fn questions_precede_deduplicated_escalations_with_stable_ids() {
    let first = decision_items(Some(&status()), &[lane(true)]);
    let second = decision_items(Some(&status()), &[lane(true)]);
    assert_eq!(first, second);
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].kind, DecisionKind::WorkerQuestion);
    assert_eq!(first[0].id, "question:cycle-1:q1");
    assert_eq!(
        first[0].contract_excerpt.as_deref(),
        Some("Do not change the public API")
    );
    assert!(first[0].answer_target.is_some());
    assert_eq!(first[1].kind, DecisionKind::Escalation);
    assert!(first[1].answer_target.is_none());
}

#[test]
fn answered_nonblocked_and_unroutable_questions_are_removed() {
    assert_eq!(decision_items(Some(&status()), &[lane(false)]).len(), 1);

    let mut active = status();
    active.tasks[0].status = TrackedTaskStatus::Active;
    assert_eq!(decision_items(Some(&active), &[lane(true)]).len(), 1);

    let mut unroutable = lane(true);
    unroutable.tasks[0].task_id = "bare-task".into();
    assert_eq!(decision_items(None, &[unroutable]).len(), 0);
}

#[test]
fn task_excerpt_falls_back_to_notes_then_title() {
    let mut with_note = status();
    with_note.tasks[0].detail = None;
    with_note.tasks[0].notes = vec!["verify with legal".into()];
    assert_eq!(
        decision_items(Some(&with_note), &[lane(true)])[0]
            .contract_excerpt
            .as_deref(),
        Some("verify with legal")
    );

    with_note.tasks[0].notes.clear();
    assert_eq!(
        decision_items(Some(&with_note), &[lane(true)])[0]
            .contract_excerpt
            .as_deref(),
        Some("Choose schema")
    );
}
