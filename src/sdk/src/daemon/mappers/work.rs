//! Recognizing the *work* inside a tool call.
//!
//! Every harness expresses its plan, its todo list, its sub-agents, and its file
//! edits as ordinary tool calls with provider-specific names and argument
//! shapes: Claude Code writes `TodoWrite`, Codex calls `update_plan`, OpenCode
//! calls `todowrite`. Folded naively they all become "ran a tool", which is why
//! a fleet view could show an agent busy for ten minutes without ever saying
//! what it was doing.
//!
//! This module is the recognizer. Given a tool call's name and input it emits
//! the extra provider-neutral work events ([`crate::harness_work::kinds`]) that
//! call implies — *in addition to* the `tool_call` event, never instead of it,
//! so the transcript is unchanged and the work surface is purely additive.

use serde_json::{Map, Value};

use crate::harness_work::kinds;

use super::events::semantic;
use super::shared::safe_stringify;
use super::types::HarnessSemanticEvent;

/// Character cap on prompt/plan text carried in a work payload. Generous enough
/// to read a delegation brief, bounded so a transcript line cannot balloon the
/// stream.
const WORK_TEXT_CAP: usize = 4000;
/// Ceiling on plan/todo items accepted from one call.
const MAX_ITEMS: usize = 200;
/// Ceiling on file paths extracted from one patch.
const MAX_PATCH_PATHS: usize = 100;

/// Every work event implied by one tool call.
///
/// Returns an empty vector for the overwhelming majority of calls — only the
/// handful of tools that carry structured intent produce anything here.
pub(super) fn work_events_for_tool(
    line: i64,
    ts: i64,
    record_type: &str,
    call_id: &str,
    tool_name: &str,
    input: &Value,
) -> Vec<HarnessSemanticEvent> {
    let lower = tool_name.trim().to_ascii_lowercase();
    let base = lower.rsplit("__").next().unwrap_or(&lower).to_string();

    if is_todo_tool(&base) {
        if let Some(items) = todo_items(input) {
            return vec![semantic(
                line,
                ts,
                record_type,
                kinds::TODO_UPDATE,
                "agent",
                serde_json::json!({ "todos": items, "source": tool_name }),
            )];
        }
    }
    if is_plan_tool(&base) {
        if let Some(event) = plan_event(line, ts, record_type, tool_name, input) {
            return vec![event];
        }
    }
    if is_subagent_tool(&base) {
        return vec![subagent_event(line, ts, record_type, call_id, input)];
    }
    file_change_events(line, ts, record_type, tool_name, &base, input)
}

/// Whether this tool writes a todo list. Covers Claude's `TodoWrite`,
/// OpenCode's `todowrite`, and the `todo_write` spelling MCP servers use.
fn is_todo_tool(base: &str) -> bool {
    base.contains("todo") && !base.contains("read")
}

/// Whether this tool declares a plan: Codex's `update_plan`, Claude's
/// `ExitPlanMode`, and the `set_plan`/`plan` spellings around them.
fn is_plan_tool(base: &str) -> bool {
    matches!(
        base,
        "update_plan" | "updateplan" | "exitplanmode" | "exit_plan_mode" | "set_plan" | "plan"
    )
}

/// Whether this tool delegates to a sub-agent: Claude's `Task`, OpenCode's
/// `task`, and the `agent`/`spawn` spellings other harnesses use.
fn is_subagent_tool(base: &str) -> bool {
    matches!(
        base,
        "task" | "agent" | "subagent" | "spawn_agent" | "dispatch_agent" | "run_agent"
    )
}

/// The todo array of a todo-writing call, normalized to a JSON array the fold
/// can read. `None` when the call carries no recognizable list — a `TodoWrite`
/// with a malformed argument should produce nothing rather than an empty list,
/// which would read as "the agent cleared its todos".
fn todo_items(input: &Value) -> Option<Value> {
    let object = input.as_object()?;
    let items = ["todos", "items", "todo", "list"]
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_array)?;
    if items.is_empty() {
        return None;
    }
    Some(Value::Array(
        items.iter().take(MAX_ITEMS).cloned().collect(),
    ))
}

/// The `plan_update` event for a plan-declaring call.
///
/// Handles both shapes in the wild: Codex passes a structured `plan` array of
/// `{ step, status }`, while Claude's `ExitPlanMode` passes one markdown string,
/// whose bullet list is parsed into steps and whose first prose line becomes the
/// goal.
fn plan_event(
    line: i64,
    ts: i64,
    record_type: &str,
    tool_name: &str,
    input: &Value,
) -> Option<HarnessSemanticEvent> {
    let object = input.as_object();
    let structured = object
        .and_then(|o| ["plan", "steps", "todos"].iter().find_map(|k| o.get(*k)))
        .and_then(Value::as_array);
    let explanation = object
        .and_then(|o| {
            ["explanation", "goal", "summary", "description"]
                .iter()
                .find_map(|k| o.get(*k))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());

    let (goal, steps) = match structured {
        Some(items) if !items.is_empty() => (
            explanation.map(str::to_string),
            items.iter().take(MAX_ITEMS).cloned().collect::<Vec<_>>(),
        ),
        _ => {
            // The prose form: one markdown plan, as ExitPlanMode sends it.
            let text = object
                .and_then(|o| {
                    ["plan", "explanation", "prompt"]
                        .iter()
                        .find_map(|k| o.get(*k))
                })
                .map(safe_stringify)
                .or_else(|| input.as_str().map(str::to_string))?;
            let (goal, steps) = parse_prose_plan(&text);
            (
                goal.or_else(|| explanation.map(str::to_string)),
                steps.into_iter().map(Value::String).collect(),
            )
        }
    };
    if goal.is_none() && steps.is_empty() {
        return None;
    }
    Some(semantic(
        line,
        ts,
        record_type,
        kinds::PLAN_UPDATE,
        "agent",
        serde_json::json!({ "goal": goal, "steps": steps, "source": tool_name }),
    ))
}

/// Split a markdown plan into its stated goal and its bullet steps.
///
/// The goal is the first non-heading prose line — what the agent says it is
/// doing — and the steps are the list items under it. A plan with no bullets
/// still yields its goal, which is the part an operator most wants on screen.
fn parse_prose_plan(text: &str) -> (Option<String>, Vec<String>) {
    let mut goal: Option<String> = None;
    let mut steps: Vec<String> = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let bullet = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| strip_numbered(trimmed));
        match bullet {
            Some(step) => {
                // Markdown task-list syntax carries completion state.
                let step = step
                    .trim()
                    .strip_prefix("[x] ")
                    .or_else(|| step.trim().strip_prefix("[ ] "))
                    .unwrap_or_else(|| step.trim());
                if !step.is_empty() && steps.len() < MAX_ITEMS {
                    steps.push(cap(step, WORK_TEXT_CAP));
                }
            }
            None if goal.is_none() => {
                let heading = trimmed.trim_start_matches('#').trim();
                if !heading.is_empty() {
                    goal = Some(cap(heading, WORK_TEXT_CAP));
                }
            }
            None => {}
        }
    }
    (goal, steps)
}

/// The text of a `1. `/`1) ` numbered list item, or `None` when the line is not
/// one.
fn strip_numbered(line: &str) -> Option<&str> {
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &line[digits.len()..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

/// The `subagent_start` event for a delegating call.
fn subagent_event(
    line: i64,
    ts: i64,
    record_type: &str,
    call_id: &str,
    input: &Value,
) -> HarnessSemanticEvent {
    let object = input.as_object().cloned().unwrap_or_default();
    let field = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|key| object.get(*key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| cap(value, WORK_TEXT_CAP))
    };
    semantic(
        line,
        ts,
        record_type,
        kinds::SUBAGENT_START,
        "agent",
        serde_json::json!({
            "call_id": call_id,
            "agent_type": field(&["subagent_type", "agentType", "agent", "type", "mode"]),
            "description": field(&["description", "title", "task"]).unwrap_or_default(),
            "prompt": field(&["prompt", "instruction", "input", "message"]),
        }),
    )
}

/// Every `file_change` event implied by a file-touching call.
///
/// Path-carrying tools (`Write`, `Edit`, `NotebookEdit`) yield one; a patch tool
/// yields one per file named in its patch body, since a single `apply_patch`
/// routinely rewrites several files and reporting it as one edit would hide the
/// rest.
fn file_change_events(
    line: i64,
    ts: i64,
    record_type: &str,
    tool_name: &str,
    base: &str,
    input: &Value,
) -> Vec<HarnessSemanticEvent> {
    let event = |path: &str, change: &str| {
        semantic(
            line,
            ts,
            record_type,
            kinds::FILE_CHANGE,
            "agent",
            serde_json::json!({
                "path": path,
                "change": change,
                "tool": tool_name,
            }),
        )
    };
    if is_patch_tool(base) {
        let body = input
            .as_object()
            .and_then(|o| {
                ["patch", "input", "diff", "content"]
                    .iter()
                    .find_map(|k| o.get(*k))
            })
            .map(safe_stringify)
            .or_else(|| input.as_str().map(str::to_string))
            .unwrap_or_default();
        return paths_in_patch(&body)
            .into_iter()
            .map(|(path, change)| event(&path, change))
            .collect();
    }
    let Some(change) = file_change_kind(base) else {
        return Vec::new();
    };
    let Some(object) = input.as_object() else {
        return Vec::new();
    };
    match file_path(object) {
        Some(path) => vec![event(&path, change)],
        None => Vec::new(),
    }
}

/// Whether this tool applies a multi-file patch rather than editing one path.
fn is_patch_tool(base: &str) -> bool {
    matches!(base, "apply_patch" | "applypatch" | "patch")
}

/// The change kind a file-touching tool implies, or `None` when the tool does
/// not write files. Read-only tools are deliberately absent: a `Read` is not a
/// change, and listing it would drown the real edits.
fn file_change_kind(base: &str) -> Option<&'static str> {
    match base {
        "write" | "create_file" | "createfile" | "new_file" => Some("added"),
        "edit" | "multiedit" | "multi_edit" | "notebookedit" | "notebook_edit"
        | "str_replace_editor" | "str_replace" | "update_file" => Some("modified"),
        "delete_file" | "deletefile" | "rm" | "remove_file" => Some("deleted"),
        _ => None,
    }
}

/// The path a file-touching call names, under any of the keys the harnesses use.
fn file_path(object: &Map<String, Value>) -> Option<String> {
    ["file_path", "filePath", "path", "notebook_path", "file"]
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
}

/// The files a `*** Add/Update/Delete File:` patch envelope names, paired with
/// the change each header declares.
fn paths_in_patch(body: &str) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for raw in body.lines() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix("*** ") else {
            continue;
        };
        let parsed = ["Add File:", "Update File:", "Delete File:"]
            .iter()
            .find_map(|prefix| rest.strip_prefix(prefix).map(|path| (*prefix, path)));
        let Some((prefix, path)) = parsed else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() || out.len() >= MAX_PATCH_PATHS {
            continue;
        }
        let change = match prefix {
            "Add File:" => "added",
            "Delete File:" => "deleted",
            _ => "modified",
        };
        out.push((path.to_string(), change));
    }
    out
}

/// Truncate `value` to `max_chars` characters, appending an ellipsis when cut.
fn cap(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let prefix: String = value.chars().take(max_chars).collect();
    format!("{prefix}…")
}
