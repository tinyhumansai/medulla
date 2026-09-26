//! Unit tests for the work fold and its data model.

use super::*;
use serde_json::json;

fn fold_with(events: &[(&str, serde_json::Value)]) -> WorkSnapshot {
    let mut fold = WorkFold::new();
    for (i, (kind, payload)) in events.iter().enumerate() {
        fold.apply(kind, payload, 1_000 + i as i64);
    }
    fold.into_snapshot()
}

#[test]
fn empty_fold_renders_nothing() {
    assert!(WorkFold::new().snapshot().is_empty());
}

#[test]
fn todo_update_replaces_the_list_and_reports_progress() {
    let snapshot = fold_with(&[
        (
            kinds::TODO_UPDATE,
            json!({ "todos": [
                { "content": "write tests", "status": "completed" },
                { "content": "ship it", "status": "pending" },
            ]}),
        ),
        (
            kinds::TODO_UPDATE,
            json!({ "todos": [
                { "content": "write tests", "status": "completed" },
                { "content": "ship it", "status": "in_progress", "activeForm": "Shipping it" },
            ]}),
        ),
    ]);
    assert_eq!(snapshot.todos.len(), 2);
    assert_eq!(snapshot.todo_progress(), (1, 2));
    let current = snapshot.current_todo().expect("one item is in progress");
    assert_eq!(current.display(), "Shipping it");
}

#[test]
fn a_repeated_todo_write_is_not_a_change() {
    let mut fold = WorkFold::new();
    let payload = json!({ "todos": [{ "content": "a", "status": "pending" }] });
    assert!(fold.apply(kinds::TODO_UPDATE, &payload, 1));
    assert!(!fold.apply(kinds::TODO_UPDATE, &payload, 2));
}

#[test]
fn cancelled_items_count_as_settled() {
    let snapshot = fold_with(&[(
        kinds::TODO_UPDATE,
        json!({ "todos": [
            { "content": "a", "status": "completed" },
            { "content": "b", "status": "cancelled" },
            { "content": "c", "status": "pending" },
        ]}),
    )]);
    assert_eq!(snapshot.todo_progress(), (2, 3));
}

#[test]
fn plan_update_records_goal_and_steps_in_either_shape() {
    let snapshot = fold_with(&[(
        kinds::PLAN_UPDATE,
        json!({
            "goal": "make the TUI show everything",
            // Codex writes bare strings and completion booleans; Claude writes
            // status strings. One parser reads both.
            "steps": ["survey the harnesses", { "step": "map them", "completed": true }],
        }),
    )]);
    assert_eq!(
        snapshot.goal.as_deref(),
        Some("make the TUI show everything")
    );
    assert_eq!(snapshot.plan.len(), 2);
    assert_eq!(snapshot.plan[0].status, WorkItemStatus::Pending);
    assert_eq!(snapshot.plan[1].status, WorkItemStatus::Completed);
}

#[test]
fn a_subagent_closes_on_the_tool_result_for_its_call() {
    let snapshot = fold_with(&[
        (
            kinds::SUBAGENT_START,
            json!({ "call_id": "t1", "agent_type": "code-reviewer", "description": "review the diff" }),
        ),
        (
            "tool_result",
            json!({ "call_id": "t1", "is_error": false, "output": "looks fine" }),
        ),
    ]);
    assert_eq!(snapshot.subagents.len(), 1);
    let run = &snapshot.subagents[0];
    assert_eq!(run.status, SubagentStatus::Done);
    assert_eq!(run.result.as_deref(), Some("looks fine"));
    assert_eq!(run.label(), "review the diff");
    assert_eq!(snapshot.running_subagents(), 0);
}

#[test]
fn a_failed_subagent_is_distinguished_from_a_finished_one() {
    let snapshot = fold_with(&[
        (
            kinds::SUBAGENT_START,
            json!({ "call_id": "t1", "description": "d" }),
        ),
        (
            "tool_result",
            json!({ "call_id": "t1", "is_error": true, "output": "boom" }),
        ),
    ]);
    assert_eq!(snapshot.subagents[0].status, SubagentStatus::Failed);
}

#[test]
fn an_unrelated_tool_result_leaves_subagents_running() {
    let snapshot = fold_with(&[
        (
            kinds::SUBAGENT_START,
            json!({ "call_id": "t1", "description": "d" }),
        ),
        (
            "tool_result",
            json!({ "call_id": "other", "is_error": false }),
        ),
    ]);
    assert_eq!(snapshot.running_subagents(), 1);
}

#[test]
fn a_respawned_call_id_is_not_recorded_twice() {
    let mut fold = WorkFold::new();
    let payload = json!({ "call_id": "t1", "description": "d" });
    assert!(fold.apply(kinds::SUBAGENT_START, &payload, 1));
    assert!(!fold.apply(kinds::SUBAGENT_START, &payload, 2));
    assert_eq!(fold.snapshot().subagents.len(), 1);
}

#[test]
fn repeat_edits_merge_onto_one_file_row() {
    let snapshot = fold_with(&[
        (
            kinds::FILE_CHANGE,
            json!({ "path": "src/a.rs", "change": "added" }),
        ),
        (
            kinds::FILE_CHANGE,
            json!({ "path": "src/a.rs", "change": "modified" }),
        ),
        (
            kinds::FILE_CHANGE,
            json!({ "path": "src/b.rs", "change": "modified" }),
        ),
    ]);
    assert_eq!(snapshot.files.len(), 2);
    assert_eq!(snapshot.files[0].edits, 2);
    // The first sighting established a new file; a later edit does not demote it.
    assert_eq!(snapshot.files[0].kind, FileChangeKind::Added);
}

#[test]
fn a_delete_overrides_an_earlier_sighting() {
    let snapshot = fold_with(&[
        (
            kinds::FILE_CHANGE,
            json!({ "path": "src/a.rs", "change": "added" }),
        ),
        (
            kinds::FILE_CHANGE,
            json!({ "path": "src/a.rs", "change": "deleted" }),
        ),
    ]);
    assert_eq!(snapshot.files[0].kind, FileChangeKind::Deleted);
}

#[test]
fn session_info_merges_rather_than_overwrites() {
    let snapshot = fold_with(&[
        (
            kinds::SESSION_INFO,
            json!({ "model": "claude-opus-5", "tools": ["Bash", "Read"] }),
        ),
        (
            kinds::SESSION_INFO,
            json!({
                "cwd": "/repo",
                "pull_request": "https://github.com/acme/repo/pull/42"
            }),
        ),
    ]);
    assert_eq!(snapshot.info.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(snapshot.info.cwd.as_deref(), Some("/repo"));
    assert_eq!(
        snapshot.info.pull_request.as_deref(),
        Some("https://github.com/acme/repo/pull/42")
    );
    assert_eq!(snapshot.info.tools, vec!["Bash", "Read"]);
}

#[test]
fn a_new_branch_clears_the_previous_branches_pull_request() {
    let snapshot = fold_with(&[
        (
            kinds::SESSION_INFO,
            json!({
                "branch": "first",
                "pull_request": "https://github.com/acme/repo/pull/42"
            }),
        ),
        (kinds::SESSION_INFO, json!({ "branch": "second" })),
    ]);

    assert_eq!(snapshot.info.branch.as_deref(), Some("second"));
    assert!(snapshot.info.pull_request.is_none());
}

#[test]
fn repeating_the_same_branch_preserves_its_pull_request() {
    let snapshot = fold_with(&[
        (
            kinds::SESSION_INFO,
            json!({
                "branch": "same",
                "pull_request": "https://github.com/acme/repo/pull/42"
            }),
        ),
        (kinds::SESSION_INFO, json!({ "branch": "same" })),
    ]);

    assert_eq!(
        snapshot.info.pull_request.as_deref(),
        Some("https://github.com/acme/repo/pull/42")
    );
}

#[test]
fn a_new_checkout_clears_the_previous_checkouts_pull_request() {
    let snapshot = fold_with(&[
        (
            kinds::SESSION_INFO,
            json!({
                "cwd": "/repo-a",
                "branch": "main",
                "pull_request": "https://github.com/acme/repo-a/pull/42"
            }),
        ),
        (
            kinds::SESSION_INFO,
            json!({ "cwd": "/repo-b", "branch": "main" }),
        ),
    ]);
    assert!(snapshot.info.pull_request.is_none());
}

#[test]
fn run_result_records_the_outcome() {
    let snapshot = fold_with(&[(
        kinds::RUN_RESULT,
        json!({ "ok": false, "duration_ms": 4200, "num_turns": 7, "cost_usd": 0.42, "summary": "failed" }),
    )]);
    let outcome = snapshot.outcome.expect("an outcome was reported");
    assert!(!outcome.ok);
    assert_eq!(outcome.duration_ms, Some(4200));
    assert_eq!(outcome.num_turns, Some(7));
    assert_eq!(outcome.summary.as_deref(), Some("failed"));
}

#[test]
fn malformed_and_unknown_payloads_are_dropped_not_guessed() {
    let mut fold = WorkFold::new();
    assert!(!fold.apply(kinds::TODO_UPDATE, &json!({ "todos": "nope" }), 1));
    assert!(!fold.apply(kinds::FILE_CHANGE, &json!({ "path": "  " }), 1));
    assert!(!fold.apply(kinds::SUBAGENT_START, &json!({}), 1));
    assert!(!fold.apply("something_new", &json!({ "a": 1 }), 1));
    assert!(fold.snapshot().is_empty());
}

#[test]
fn work_kinds_are_recognized_by_name() {
    assert!(kinds::is_work_kind(kinds::TODO_UPDATE));
    assert!(!kinds::is_work_kind("tool_call"));
}

#[test]
fn a_snapshot_round_trips_through_json() {
    let snapshot = fold_with(&[
        (
            kinds::TODO_UPDATE,
            json!({ "todos": [{ "content": "a", "status": "in_progress" }] }),
        ),
        (
            kinds::SUBAGENT_START,
            json!({ "call_id": "t1", "description": "d" }),
        ),
    ]);
    let text = serde_json::to_string(&snapshot).expect("snapshot serializes");
    let parsed: WorkSnapshot = serde_json::from_str(&text).expect("snapshot parses");
    assert_eq!(parsed, snapshot);
}
