//! Tests for the work recognizer: the todo lists, plans, sub-agents, file
//! edits, and session facts each provider's transcript carries, and the folded
//! snapshot they add up to.

use serde_json::json;

use crate::harness_work::{kinds, FileChangeKind, SubagentStatus, WorkFold, WorkItemStatus};

use super::types::HarnessSemanticEvent;
use super::HarnessLineMapper;

/// Map every line of a transcript, in order, as one stream.
fn map_all(provider: &str, lines: &[serde_json::Value]) -> Vec<HarnessSemanticEvent> {
    let mut mapper = HarnessLineMapper::new(provider);
    lines
        .iter()
        .enumerate()
        .flat_map(|(i, line)| mapper.map_line(&line.to_string(), i as i64))
        .collect()
}

/// The payload of the first event of `kind`, or a panic naming what was missing.
fn payload_of<'a>(events: &'a [HarnessSemanticEvent], kind: &str) -> &'a serde_json::Value {
    events
        .iter()
        .find(|event| event.event.kind == kind)
        .map(|event| &event.event.payload)
        .unwrap_or_else(|| panic!("no {kind} event in {:?}", kinds_of(events)))
}

fn kinds_of(events: &[HarnessSemanticEvent]) -> Vec<&str> {
    events.iter().map(|e| e.event.kind.as_str()).collect()
}

/// Fold a mapped stream the way a consumer does.
fn fold(events: &[HarnessSemanticEvent]) -> crate::harness_work::WorkSnapshot {
    let mut fold = WorkFold::new();
    for event in events {
        fold.apply(&event.event.kind, &event.event.payload, event.timestamp_ms);
    }
    fold.into_snapshot()
}

/// A Claude `assistant` record wrapping one tool_use block.
fn claude_tool_use(id: &str, name: &str, input: serde_json::Value) -> serde_json::Value {
    json!({
        "type": "assistant",
        "message": { "role": "assistant", "content": [
            { "type": "tool_use", "id": id, "name": name, "input": input }
        ]}
    })
}

#[test]
fn claude_todo_write_yields_both_a_tool_call_and_a_todo_update() {
    let events = map_all(
        "claude",
        &[claude_tool_use(
            "c1",
            "TodoWrite",
            json!({ "todos": [
                { "content": "read the mappers", "status": "completed", "activeForm": "Reading the mappers" },
                { "content": "teach them todos", "status": "in_progress", "activeForm": "Teaching them todos" },
            ]}),
        )],
    );
    // The transcript is unchanged — the work event is additive.
    assert_eq!(kinds_of(&events), vec!["tool_call", kinds::TODO_UPDATE]);

    let snapshot = fold(&events);
    assert_eq!(snapshot.todo_progress(), (1, 2));
    assert_eq!(
        snapshot.current_todo().map(|t| t.display()),
        Some("Teaching them todos")
    );
}

#[test]
fn claude_task_tool_opens_a_subagent_that_its_result_closes() {
    let events = map_all(
        "claude",
        &[
            claude_tool_use(
                "t1",
                "Task",
                json!({
                    "subagent_type": "code-reviewer",
                    "description": "review the diff",
                    "prompt": "Look at the branch and report findings.",
                }),
            ),
            json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "two findings" }
                ]}
            }),
        ],
    );
    let start = payload_of(&events, kinds::SUBAGENT_START);
    assert_eq!(start["agent_type"], "code-reviewer");
    assert_eq!(start["description"], "review the diff");

    let snapshot = fold(&events);
    assert_eq!(snapshot.subagents.len(), 1);
    assert_eq!(snapshot.subagents[0].status, SubagentStatus::Done);
    assert_eq!(
        snapshot.subagents[0].result.as_deref(),
        Some("two findings")
    );
}

#[test]
fn claude_worktree_report_moves_the_session_to_the_created_branch() {
    let events = map_all(
        "claude",
        &[json!({
            "type": "user",
            "message": { "role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "worktree-1",
                "content": "[PASS] WORKTREE_READY\n  repository: /repo\n  path: /repo/worktrees/fix-label\n  branch: fix-label\n  head: abc1234\n  created: true\n  next: cd /repo/worktrees/fix-label"
            }]}
        })],
    );
    let snapshot = fold(&events);
    assert_eq!(
        snapshot.info.cwd.as_deref(),
        Some("/repo/worktrees/fix-label")
    );
    assert_eq!(snapshot.info.branch.as_deref(), Some("fix-label"));
}

#[test]
fn claude_worktree_report_survives_a_later_command_failure() {
    let events = map_all(
        "claude",
        &[json!({
            "type": "user",
            "message": { "role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "worktree-1",
                "is_error": true,
                "content": "[PASS] WORKTREE_READY\n  path: /repo/worktrees/fix-label\n  branch: fix-label\nerror: tests failed"
            }]}
        })],
    );
    let snapshot = fold(&events);
    assert_eq!(
        snapshot.info.cwd.as_deref(),
        Some("/repo/worktrees/fix-label")
    );
    assert_eq!(snapshot.info.branch.as_deref(), Some("fix-label"));
}

#[test]
fn text_worktree_report_ignores_fields_before_its_ready_marker() {
    let events = map_all(
        "claude",
        &[json!({
            "type": "user",
            "message": { "role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "worktree-1",
                "content": concat!(
                    "path: /unrelated\nbranch: stale\n",
                    "[PASS] WORKTREE_READY\n",
                    "  path: /repo/worktrees/fix-label\n",
                    "  branch: fix-label\n",
                    "  head: abc1234\n",
                    "later output"
                )
            }]}
        })],
    );
    let snapshot = fold(&events);
    assert_eq!(
        snapshot.info.cwd.as_deref(),
        Some("/repo/worktrees/fix-label")
    );
    assert_eq!(snapshot.info.branch.as_deref(), Some("fix-label"));
}

#[test]
fn claude_exit_plan_mode_becomes_a_goal_and_steps() {
    let events = map_all(
        "claude",
        &[claude_tool_use(
            "p1",
            "ExitPlanMode",
            json!({ "plan": "## Enrich the TUI\nHere is the plan:\n- parse the harnesses\n- fold the work\n2. render it\n" }),
        )],
    );
    let snapshot = fold(&events);
    assert_eq!(snapshot.goal.as_deref(), Some("Enrich the TUI"));
    let steps: Vec<&str> = snapshot.plan.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(
        steps,
        vec!["parse the harnesses", "fold the work", "render it"]
    );
}

#[test]
fn claude_edits_and_writes_are_recorded_as_file_changes() {
    let events = map_all(
        "claude",
        &[
            claude_tool_use("w1", "Write", json!({ "file_path": "src/new.rs" })),
            claude_tool_use("e1", "Edit", json!({ "file_path": "src/new.rs" })),
            claude_tool_use("r1", "Read", json!({ "file_path": "src/other.rs" })),
        ],
    );
    let snapshot = fold(&events);
    // A read is not a change: only the two writes land.
    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(snapshot.files[0].path, "src/new.rs");
    assert_eq!(snapshot.files[0].kind, FileChangeKind::Added);
    assert_eq!(snapshot.files[0].edits, 2);
}

#[test]
fn claude_init_and_result_records_describe_the_run() {
    let events = map_all(
        "claude",
        &[
            json!({
                "type": "system",
                "subtype": "init",
                "model": "claude-opus-5",
                "cwd": "/repo",
                "session_id": "s-1",
                "permissionMode": "acceptEdits",
                "tools": ["Bash", "Read"],
                "slash_commands": ["review"],
                "mcp_servers": [{ "name": "langfuse", "status": "connected" }],
            }),
            json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "duration_ms": 8123,
                "num_turns": 4,
                "total_cost_usd": 0.19,
                "result": "done",
            }),
        ],
    );
    let snapshot = fold(&events);
    assert_eq!(snapshot.info.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(snapshot.info.cwd.as_deref(), Some("/repo"));
    assert_eq!(
        snapshot.info.permission_mode.as_deref(),
        Some("acceptEdits")
    );
    assert_eq!(snapshot.info.mcp_servers, vec!["langfuse"]);
    let outcome = snapshot.outcome.expect("the run reported an outcome");
    assert!(outcome.ok);
    assert_eq!(outcome.num_turns, Some(4));
    assert_eq!(outcome.cost_usd, Some(0.19));
}

#[test]
fn a_system_record_that_is_not_init_stays_ignored() {
    let events = map_all(
        "claude",
        &[json!({ "type": "system", "subtype": "warning" })],
    );
    assert!(events.is_empty());
}

#[test]
fn codex_todo_list_items_fold_with_their_completion_flags() {
    let events = map_all(
        "codex",
        &[json!({
            "type": "item.completed",
            "item": { "type": "todo_list", "id": "i1", "items": [
                { "text": "survey", "completed": true },
                { "text": "implement", "completed": false },
            ]}
        })],
    );
    let snapshot = fold(&events);
    assert_eq!(snapshot.todos.len(), 2);
    assert_eq!(snapshot.todos[0].status, WorkItemStatus::Completed);
    assert_eq!(snapshot.todos[1].status, WorkItemStatus::Pending);
}

#[test]
fn codex_json_worktree_report_moves_the_session_to_the_created_branch() {
    let events = map_all(
        "codex",
        &[json!({
            "type": "item.completed",
            "item": {
                "type": "command_execution",
                "id": "cmd-1",
                "command": "worktree fix-label --json",
                "aggregated_output": concat!(
                    "initializing\n{",
                    "\"status\":\"ready\",",
                    "\"repository\":\"/repo\",",
                    "\"path\":\"/repo/worktrees/fix-label\",",
                    "\"branch\":\"fix-label\",",
                    "\"head\":\"abc123456789\",",
                    "\"headShort\":\"abc1234\",",
                    "\"created\":true,",
                    "\"submodules\":{\"state\":\"initialized_recursive\",\"count\":0},",
                    "\"nextCommand\":\"cd /repo/worktrees/fix-label\"}"
                ),
                "exit_code": 0
            }
        })],
    );
    let snapshot = fold(&events);
    assert_eq!(
        snapshot.info.cwd.as_deref(),
        Some("/repo/worktrees/fix-label")
    );
    assert_eq!(snapshot.info.branch.as_deref(), Some("fix-label"));
}

#[test]
fn codex_worktree_report_survives_trailing_output_and_a_later_failure() {
    let events = map_all(
        "codex",
        &[json!({
            "type": "item.completed",
            "item": {
                "type": "command_execution",
                "id": "cmd-1",
                "command": "worktree fix-label --json && cargo test",
                "aggregated_output": concat!(
                    "{\"status\":\"ready\",\"repository\":\"/repo\",",
                    "\"path\":\"/repo/worktrees/fix-label\",\"branch\":\"fix-label\",",
                    "\"head\":\"abc123456789\",\"headShort\":\"abc1234\",",
                    "\"created\":true,\"submodules\":{",
                    "\"state\":\"initialized_recursive\",\"count\":0},",
                    "\"nextCommand\":\"cd /repo/worktrees/fix-label\"}\n",
                    "error: tests failed"
                ),
                "exit_code": 1
            }
        })],
    );
    let snapshot = fold(&events);
    assert_eq!(
        snapshot.info.cwd.as_deref(),
        Some("/repo/worktrees/fix-label")
    );
    assert_eq!(snapshot.info.branch.as_deref(), Some("fix-label"));
}

#[test]
fn unrelated_command_output_does_not_move_the_session() {
    let events = map_all(
        "codex",
        &[
            json!({
                "type": "item.completed",
                "item": {
                    "type": "command_execution",
                    "id": "cmd-1",
                    "aggregated_output": "[FAIL] worktree creation failed",
                    "exit_code": 1
                }
            }),
            json!({
                "type": "item.completed",
                "item": {
                    "type": "command_execution",
                    "id": "cmd-2",
                    "aggregated_output": "path: /also-wrong\nbranch: wrong",
                    "exit_code": 0
                }
            }),
            json!({
                "type": "item.completed",
                "item": {
                    "type": "command_execution",
                    "id": "cmd-3",
                    "aggregated_output": "{\"status\":\"ready\",\"path\":\"/deploy\",\"branch\":\"prod\"}",
                    "exit_code": 0
                }
            }),
        ],
    );
    assert!(fold(&events).info.is_empty());
}

#[test]
fn codex_file_change_items_list_every_path() {
    let events = map_all(
        "codex",
        &[json!({
            "type": "item.completed",
            "item": { "type": "file_change", "id": "i2", "changes": [
                { "path": "src/a.rs", "kind": "add" },
                { "path": "src/b.rs", "kind": "delete" },
            ]}
        })],
    );
    let snapshot = fold(&events);
    assert_eq!(snapshot.files.len(), 2);
    assert_eq!(snapshot.files[0].kind, FileChangeKind::Added);
    assert_eq!(snapshot.files[1].kind, FileChangeKind::Deleted);
}

#[test]
fn codex_update_plan_calls_declare_the_plan() {
    let events = map_all(
        "codex",
        &[json!({
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "update_plan",
                "call_id": "f1",
                "arguments": "{\"explanation\":\"ship the surface\",\"plan\":[{\"step\":\"map\",\"status\":\"completed\"},{\"step\":\"render\",\"status\":\"in_progress\"}]}",
            }
        })],
    );
    let snapshot = fold(&events);
    assert_eq!(snapshot.goal.as_deref(), Some("ship the surface"));
    assert_eq!(snapshot.plan.len(), 2);
    assert_eq!(snapshot.plan[1].status, WorkItemStatus::InProgress);
}

#[test]
fn codex_apply_patch_reports_every_file_in_the_patch() {
    let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new\n*** Add File: src/b.rs\n+fresh\n*** End Patch";
    let events = map_all(
        "codex",
        &[json!({
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "apply_patch",
                "call_id": "f2",
                "arguments": json!({ "patch": patch }).to_string(),
            }
        })],
    );
    let snapshot = fold(&events);
    let paths: Vec<&str> = snapshot.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["src/a.rs", "src/b.rs"]);
    assert_eq!(snapshot.files[1].kind, FileChangeKind::Added);
}

#[test]
fn codex_mcp_and_web_search_items_are_no_longer_dropped() {
    let events = map_all(
        "codex",
        &[
            json!({
                "type": "item.started",
                "item": { "type": "mcp_tool_call", "id": "m1", "server": "langfuse", "tool": "getPrompt", "arguments": { "name": "p" } }
            }),
            json!({
                "type": "item.completed",
                "item": { "type": "mcp_tool_call", "id": "m1", "status": "completed", "result": "ok" }
            }),
            json!({
                "type": "item.started",
                "item": { "type": "web_search", "id": "w1", "query": "ratatui tables" }
            }),
        ],
    );
    assert_eq!(
        kinds_of(&events),
        vec!["tool_call", "tool_result", "tool_call"]
    );
    let call = payload_of(&events, "tool_call");
    assert_eq!(call["tool_name"], "langfuse__getPrompt");
    assert_eq!(call["tool_kind"], "mcp");
}

#[test]
fn codex_error_items_surface_as_errors() {
    let events = map_all(
        "codex",
        &[json!({
            "type": "item.completed",
            "item": { "type": "error", "id": "e1", "message": "context window exceeded" }
        })],
    );
    assert_eq!(kinds_of(&events), vec!["error"]);
    assert_eq!(
        payload_of(&events, "error")["message"],
        "context window exceeded"
    );
}

#[test]
fn opencode_todowrite_and_task_tools_are_recognized() {
    let events = map_all(
        "opencode",
        &[
            json!({
                "type": "tool",
                "part": { "tool": "todowrite", "callID": "o1", "state": {
                    "status": "running",
                    "input": { "todos": [{ "content": "one", "status": "pending" }] }
                }}
            }),
            json!({
                "type": "tool",
                "part": { "tool": "task", "callID": "o2", "state": {
                    "status": "running",
                    "input": { "description": "spin up a helper", "prompt": "go" }
                }}
            }),
        ],
    );
    let snapshot = fold(&events);
    assert_eq!(snapshot.todos.len(), 1);
    assert_eq!(snapshot.subagents.len(), 1);
    assert_eq!(snapshot.subagents[0].label(), "spin up a helper");
    assert_eq!(snapshot.running_subagents(), 1);
}

#[test]
fn an_mcp_wrapped_todo_tool_still_reads_as_todos() {
    // MCP prefixes the tool name; the recognizer matches on the bare name so a
    // proxied harness is not invisible.
    let events = map_all(
        "claude",
        &[claude_tool_use(
            "m1",
            "mcp__harness__todowrite",
            json!({ "todos": [{ "content": "x", "status": "pending" }] }),
        )],
    );
    let snapshot = fold(&events);
    assert_eq!(snapshot.todos.len(), 1);
}

#[test]
fn an_ordinary_tool_call_produces_no_work_events() {
    let events = map_all(
        "claude",
        &[claude_tool_use("b1", "Bash", json!({ "command": "ls" }))],
    );
    assert_eq!(kinds_of(&events), vec!["tool_call"]);
    assert!(fold(&events).is_empty());
}

#[test]
fn a_malformed_todo_write_does_not_claim_an_empty_list() {
    let events = map_all(
        "claude",
        &[claude_tool_use("b2", "TodoWrite", json!({ "todos": [] }))],
    );
    assert_eq!(kinds_of(&events), vec!["tool_call"]);
}
