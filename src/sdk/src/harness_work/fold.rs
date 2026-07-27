//! The fold: turn a session's semantic-event stream into a live
//! [`WorkSnapshot`].
//!
//! Stateful and order-dependent by design — a todo list is a latest-write-wins
//! surface, a sub-agent's result arrives long after its spawn, and a file is
//! touched repeatedly. Feed events in stream order; the snapshot is always the
//! current answer to "what is this harness doing".
//!
//! The fold is deliberately tolerant: an event whose payload does not match its
//! kind is dropped rather than guessed at, and an unknown kind is ignored. A
//! harness that streams something we have not taught yet must never corrupt the
//! part of the picture we do understand.

use serde_json::Value;

use super::kinds;
use super::types::{
    FileChange, FileChangeKind, RunOutcome, SubagentRun, SubagentStatus, TodoItem, WorkItemStatus,
    WorkSnapshot,
};

/// How many sub-agent runs to retain. A fan-out this wide is already beyond
/// what any screen shows; retaining more would only grow memory for a session
/// nobody is reading.
const MAX_SUBAGENTS: usize = 64;
/// How many distinct touched files to retain.
const MAX_FILES: usize = 200;
/// How many todo items to accept from one write.
const MAX_TODOS: usize = 200;
/// Character cap on free text folded into the snapshot (goals, results).
const TEXT_CAP: usize = 2000;

/// Accumulates a [`WorkSnapshot`] from a session's semantic events.
///
/// Create one per session — the sub-agent and file state must not leak between
/// two harnesses' streams.
#[derive(Debug, Clone, Default)]
pub struct WorkFold {
    snapshot: WorkSnapshot,
}

impl WorkFold {
    /// A fold over an empty session.
    pub fn new() -> Self {
        WorkFold::default()
    }

    /// Resume a fold from a snapshot taken earlier.
    ///
    /// For consumers that keep the snapshot rather than the fold — the
    /// receiver-side session view does exactly this, so each arriving envelope
    /// continues the same accumulation instead of starting over.
    pub fn from_snapshot(snapshot: WorkSnapshot) -> Self {
        WorkFold { snapshot }
    }

    /// The snapshot as of every event applied so far.
    pub fn snapshot(&self) -> &WorkSnapshot {
        &self.snapshot
    }

    /// Take the snapshot, leaving the fold empty.
    pub fn into_snapshot(self) -> WorkSnapshot {
        self.snapshot
    }

    /// Fold one semantic event in, identified by its wire `kind` and `payload`.
    /// `at` is the event's epoch-ms timestamp, used to stamp sub-agent and file
    /// activity.
    ///
    /// Returns `true` when the snapshot changed, so a caller can publish only on
    /// real movement rather than on every line of a transcript.
    pub fn apply(&mut self, kind: &str, payload: &Value, at: i64) -> bool {
        match kind {
            kinds::TODO_UPDATE => self.apply_todos(payload),
            kinds::PLAN_UPDATE => self.apply_plan(payload),
            kinds::SUBAGENT_START => self.apply_subagent_start(payload, at),
            kinds::FILE_CHANGE => self.apply_file_change(payload, at),
            kinds::SESSION_INFO => self.apply_session_info(payload),
            kinds::RUN_RESULT => self.apply_run_result(payload),
            // A sub-agent's return is an ordinary tool_result on the spawning
            // call id. Closing the run here rather than in the mappers keeps
            // them stateless: a single transcript line never knows whether the
            // id it is answering was a delegation.
            "tool_result" => self.apply_tool_result(payload, at),
            _ => false,
        }
    }

    /// Fold in a [`HarnessEvent`](::tinyplace::types::HarnessEvent) directly,
    /// reading its kind, payload, and ISO timestamp.
    pub fn apply_event(&mut self, event: &::tinyplace::types::HarnessEvent, at: i64) -> bool {
        self.apply(&event.kind, &event.payload, at)
    }

    /// Replace the todo list from a `todo_update` payload.
    fn apply_todos(&mut self, payload: &Value) -> bool {
        let Some(items) = payload.get("todos").and_then(Value::as_array) else {
            return false;
        };
        let todos: Vec<TodoItem> = items
            .iter()
            .filter_map(todo_from_value)
            .take(MAX_TODOS)
            .collect();
        if todos == self.snapshot.todos {
            return false;
        }
        self.snapshot.todos = todos;
        true
    }

    /// Replace the plan (and the goal it states) from a `plan_update` payload.
    fn apply_plan(&mut self, payload: &Value) -> bool {
        let mut changed = false;
        if let Some(goal) = payload.get("goal").and_then(Value::as_str) {
            let goal = cap(goal.trim(), TEXT_CAP);
            if !goal.is_empty() && self.snapshot.goal.as_deref() != Some(goal.as_str()) {
                self.snapshot.goal = Some(goal);
                changed = true;
            }
        }
        if let Some(items) = payload.get("steps").and_then(Value::as_array) {
            let plan: Vec<TodoItem> = items
                .iter()
                .filter_map(todo_from_value)
                .take(MAX_TODOS)
                .collect();
            if plan != self.snapshot.plan {
                self.snapshot.plan = plan;
                changed = true;
            }
        }
        changed
    }

    /// Record a spawned sub-agent from a `subagent_start` payload.
    fn apply_subagent_start(&mut self, payload: &Value, at: i64) -> bool {
        let call_id = payload
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let description = payload
            .get("description")
            .and_then(Value::as_str)
            .map(|s| cap(s.trim(), TEXT_CAP))
            .unwrap_or_default();
        // A spawn with neither a handle nor a description says nothing a screen
        // could show, and would render as a blank row.
        if call_id.is_empty() && description.is_empty() {
            return false;
        }
        if !call_id.is_empty() && self.snapshot.subagents.iter().any(|s| s.call_id == call_id) {
            return false; // a resend of a spawn we already have
        }
        self.snapshot.subagents.push(SubagentRun {
            call_id,
            agent_type: text_field(payload, "agent_type"),
            description,
            prompt: text_field(payload, "prompt").map(|p| cap(&p, TEXT_CAP)),
            status: SubagentStatus::Running,
            result: None,
            started_at: at,
            ended_at: None,
        });
        // Drop the oldest settled runs first: a finished sub-agent is history,
        // a running one is the thing the operator is waiting on.
        while self.snapshot.subagents.len() > MAX_SUBAGENTS {
            let victim = self
                .snapshot
                .subagents
                .iter()
                .position(|s| s.status != SubagentStatus::Running)
                .unwrap_or(0);
            self.snapshot.subagents.remove(victim);
        }
        true
    }

    /// Close a running sub-agent when its spawning call's result arrives.
    fn apply_tool_result(&mut self, payload: &Value, at: i64) -> bool {
        let Some(call_id) = payload.get("call_id").and_then(Value::as_str) else {
            return false;
        };
        if call_id.is_empty() {
            return false;
        }
        let Some(run) = self
            .snapshot
            .subagents
            .iter_mut()
            .find(|s| s.call_id == call_id && s.status == SubagentStatus::Running)
        else {
            return false;
        };
        let is_error = payload
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        run.status = if is_error {
            SubagentStatus::Failed
        } else {
            SubagentStatus::Done
        };
        run.result = payload
            .get("output")
            .and_then(Value::as_str)
            .map(|o| cap(o.trim(), TEXT_CAP))
            .filter(|o| !o.is_empty());
        run.ended_at = Some(at);
        true
    }

    /// Fold one touched file into the file list, merging repeat edits.
    fn apply_file_change(&mut self, payload: &Value, at: i64) -> bool {
        let path = payload
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let Some(path) = path else { return false };
        let kind = payload
            .get("change")
            .and_then(Value::as_str)
            .map(FileChangeKind::from_wire)
            .unwrap_or_default();
        let additions = payload.get("additions").and_then(Value::as_i64);
        let deletions = payload.get("deletions").and_then(Value::as_i64);

        if let Some(existing) = self.snapshot.files.iter_mut().find(|f| f.path == path) {
            existing.edits += 1;
            existing.at = at;
            // A file created then edited is still, to a reader, a new file: only
            // a later delete overrides what the first sighting established.
            if kind == FileChangeKind::Deleted {
                existing.kind = FileChangeKind::Deleted;
            }
            if additions.is_some() {
                existing.additions = additions;
            }
            if deletions.is_some() {
                existing.deletions = deletions;
            }
            return true;
        }
        self.snapshot.files.push(FileChange {
            path: path.to_string(),
            kind,
            edits: 1,
            additions,
            deletions,
            at,
        });
        while self.snapshot.files.len() > MAX_FILES {
            self.snapshot.files.remove(0);
        }
        true
    }

    /// Merge a `session_info` payload into what the harness said about itself.
    fn apply_session_info(&mut self, payload: &Value) -> bool {
        let before = self.snapshot.info.clone();
        let info = &mut self.snapshot.info;
        merge_text(&mut info.model, payload, "model");
        merge_text(&mut info.cwd, payload, "cwd");
        merge_text(&mut info.session_id, payload, "session_id");
        merge_text(&mut info.permission_mode, payload, "permission_mode");
        merge_list(&mut info.tools, payload, "tools");
        merge_list(&mut info.slash_commands, payload, "slash_commands");
        merge_list(&mut info.mcp_servers, payload, "mcp_servers");
        *info != before
    }

    /// Record how the run ended from a `run_result` payload.
    fn apply_run_result(&mut self, payload: &Value) -> bool {
        let outcome = RunOutcome {
            ok: payload.get("ok").and_then(Value::as_bool).unwrap_or(true),
            duration_ms: payload.get("duration_ms").and_then(Value::as_i64),
            num_turns: payload.get("num_turns").and_then(Value::as_i64),
            cost_usd: payload.get("cost_usd").and_then(Value::as_f64),
            summary: text_field(payload, "summary").map(|s| cap(&s, TEXT_CAP)),
        };
        if self.snapshot.outcome.as_ref() == Some(&outcome) {
            return false;
        }
        self.snapshot.outcome = Some(outcome);
        true
    }
}

/// Parse one todo item from its wire object. Accepts the `content` key Claude
/// Code writes and the `text`/`title`/`step` spellings the other harnesses use,
/// so one parser covers every provider. `None` for an item with no text at all.
fn todo_from_value(value: &Value) -> Option<TodoItem> {
    // Codex writes plain strings for plan steps; treat one as a pending item.
    if let Some(text) = value.as_str() {
        let text = text.trim();
        return (!text.is_empty()).then(|| TodoItem {
            text: cap(text, TEXT_CAP),
            status: WorkItemStatus::Pending,
            active_form: None,
        });
    }
    let object = value.as_object()?;
    let text = ["content", "text", "title", "step", "task", "description"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|t| !t.is_empty())?;
    // Codex marks a completed plan step with a boolean rather than a status.
    let status = match object.get("status").and_then(Value::as_str) {
        Some(raw) => WorkItemStatus::from_wire(raw),
        None => match object.get("completed").and_then(Value::as_bool) {
            Some(true) => WorkItemStatus::Completed,
            _ => WorkItemStatus::Pending,
        },
    };
    Some(TodoItem {
        text: cap(text, TEXT_CAP),
        status,
        active_form: ["activeForm", "active_form"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(|f| cap(f, TEXT_CAP)),
    })
}

/// A trimmed, non-empty string field of `payload`, or `None`.
fn text_field(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Set `slot` from `payload[key]` when the payload reports a non-empty value.
/// Absent means "not reported" and must not erase what an earlier event said.
fn merge_text(slot: &mut Option<String>, payload: &Value, key: &str) {
    if let Some(value) = text_field(payload, key) {
        *slot = Some(value);
    }
}

/// Replace `slot` from `payload[key]` when the payload carries a non-empty
/// string array. Same "absent ≠ empty" rule as [`merge_text`].
fn merge_list(slot: &mut Vec<String>, payload: &Value, key: &str) {
    let Some(items) = payload.get(key).and_then(Value::as_array) else {
        return;
    };
    let values: Vec<String> = items
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect();
    if !values.is_empty() {
        *slot = values;
    }
}

/// Truncate `value` to `max_chars` characters, appending an ellipsis when cut.
fn cap(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let prefix: String = value.chars().take(max_chars).collect();
    format!("{prefix}…")
}
