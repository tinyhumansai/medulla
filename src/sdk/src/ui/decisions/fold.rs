//! Deterministic folding of current harness/lane state into prepared decisions.

use std::collections::BTreeSet;

use crate::harness_contract::{HarnessStatus, TrackedTask, TrackedTaskStatus};
use crate::ui::agents::{parse_task_key, AgentLane};

use super::types::{DecisionAnswerTarget, DecisionItem, DecisionKind};

/// Small deterministic hash for stable escalation ids without a crypto dependency.
fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

/// Locate a harness task corresponding to a folded delegated task.
fn tracked_task<'a>(
    status: Option<&'a HarnessStatus>,
    full_task_id: &str,
    bare_task_id: &str,
) -> Option<&'a TrackedTask> {
    status?.tasks.iter().find(|task| {
        task.id == bare_task_id
            || task.id == full_task_id
            || task
                .delegated_task_ids
                .iter()
                .any(|id| id == bare_task_id || id == full_task_id)
    })
}

/// Best human-readable task boundary available before `WorkerContract` lands.
fn task_excerpt(task: Option<&TrackedTask>) -> Option<String> {
    let task = task?;
    task.detail
        .clone()
        .or_else(|| task.notes.last().cloned())
        .or_else(|| Some(task.title.clone()))
}

/// Fold harness escalations and pending lane questions into one stable queue.
///
/// Questions are ordered by lane/task order before free-form escalations.
/// Duplicate escalation strings collapse to one item. A question disappears as
/// soon as the lane fold clears its `question_id`/attention, which is the
/// answered-item removal path.
pub fn decision_items(status: Option<&HarnessStatus>, lanes: &[AgentLane]) -> Vec<DecisionItem> {
    let mut items = Vec::new();

    for lane in lanes {
        for task in &lane.tasks {
            let (Some(question_id), Some(question)) =
                (task.question_id.as_ref(), task.attention.as_ref())
            else {
                continue;
            };
            let (cycle_id, bare_task_id) = parse_task_key(&task.task_id);
            let Some(cycle_id) = cycle_id else { continue };
            let tracked = tracked_task(status, &task.task_id, bare_task_id);
            if tracked.is_some_and(|task| task.status != TrackedTaskStatus::Blocked) {
                continue;
            }
            items.push(DecisionItem {
                id: format!("question:{cycle_id}:{question_id}"),
                kind: DecisionKind::WorkerQuestion,
                question: question.clone(),
                lane_context: format!("{} · {}", lane.label, bare_task_id),
                contract_excerpt: task_excerpt(tracked),
                answer_target: Some(DecisionAnswerTarget {
                    cycle_id: cycle_id.to_string(),
                    question_id: question_id.clone(),
                }),
            });
        }
    }

    let mut seen = BTreeSet::new();
    if let Some(status) = status {
        for escalation in &status.escalations {
            let message = escalation.trim();
            if message.is_empty() || !seen.insert(message.to_string()) {
                continue;
            }
            items.push(DecisionItem {
                id: format!("escalation:{:016x}", stable_hash(message)),
                kind: DecisionKind::Escalation,
                question: message.to_string(),
                lane_context: "harness escalation".into(),
                contract_excerpt: None,
                answer_target: None,
            });
        }
    }
    items
}
