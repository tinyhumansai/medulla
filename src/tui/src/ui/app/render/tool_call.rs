//! One-line, human-readable summaries of orchestrator tool calls.
//!
//! The transcript is read by a person watching work happen, not by someone
//! auditing a wire log. A raw call renders as
//! `⏺ tool({"managers": [{"id":"news-fetch","name":"News Fetcher","ob…` — the
//! name missing, the arguments a truncated JSON blob. This module turns the
//! same call into `⏺ deploying manager News Fetcher — Fetch today's headlines`.
//!
//! Two rules hold for every tool, known or not:
//!
//! - **Never print raw JSON.** Arrays and objects are counted or named, never
//!   dumped. A blob that overflows the line teaches the reader nothing.
//! - **Never invent.** An argument that isn't there is omitted; the verb alone
//!   is still useful, and a placeholder would read as a value the model chose.

use serde_json::Value;

/// The longest fragment of a free-text argument (an objective, a query) to
/// carry inline before eliding. Long enough to identify the work, short enough
/// to leave the verb readable on a narrow pane.
const DETAIL_MAX: usize = 60;

/// The tool calls carried by a raw `inference_end` payload, as `(name, args)`.
///
/// The backend runtime surfaces `inference_end` untyped, so its `toolCalls`
/// array is read straight from the JSON. `args` is accepted either as an object
/// or as a JSON-encoded string — providers differ, and a string that does not
/// parse is passed through as a plain scalar rather than dropped.
pub(super) fn calls_from_payload(data: &serde_json::Map<String, Value>) -> Vec<(String, Value)> {
    let Some(calls) = data.get("toolCalls").and_then(Value::as_array) else {
        return Vec::new();
    };
    calls
        .iter()
        .filter_map(|call| {
            let obj = call.as_object()?;
            let name = obj.get("name").and_then(Value::as_str).unwrap_or("").trim();
            let args = match obj.get("args") {
                Some(Value::String(raw)) => {
                    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone()))
                }
                Some(other) => other.clone(),
                None => Value::Null,
            };
            // A call with neither a name nor arguments describes nothing.
            (!name.is_empty() || !args.is_null()).then(|| (name.to_string(), args))
        })
        .collect()
}

/// A human-readable one-line summary of a tool call.
///
/// `args` is the call's arguments as parsed JSON — `Value::Null` when the tool
/// takes none, or when a half-streamed fragment never parsed.
pub(super) fn summarize(name: &str, args: &Value) -> String {
    let name = name.trim();
    if name.is_empty() {
        // A call whose name never arrived. The generic verb is honest; naming
        // it "tool" (the old behaviour) reads like a tool actually called that.
        return "calling a tool".to_string();
    }
    match name {
        // Ledger and capacity reads: the verb is the whole story.
        "host_list" => "listing hosts".into(),
        "agent_list" => "listing agents".into(),
        "harness_list" => "listing harnesses".into(),
        "manager_list" => "listing managers".into(),
        "agent_status" => "checking task status".into(),
        "current_time" => "checking the time".into(),

        // Environment paging.
        "context_list" => "reading the environment index".into(),
        "context_search" => match text(args, &["query", "needle", "substring"]) {
            Some(q) => format!("searching the environment for \"{}\"", clip(&q)),
            None => "searching the environment".into(),
        },
        "context_peek" => match text(args, &["ref", "chunkRef", "id"]) {
            Some(r) => format!("reading {}", clip(&r)),
            None => "reading the environment".into(),
        },
        "context_summarize" => match text(args, &["ref", "chunkRef", "id"]) {
            Some(r) => format!("summarizing {}", clip(&r)),
            None => "summarizing the environment".into(),
        },

        // The fan-out verbs, where the interesting part is nested in an array.
        "deploy_managers" => deploy_summary(args),
        "delegate_tasks" => delegate_summary(args),

        // Targeted operations on one manager or agent.
        "manager_send" => with_target(args, "steering", &["managerId", "id"]),
        "manager_wait" => with_target(args, "waiting on", &["managerId", "id"]),
        "manager_stop" => with_target(args, "stopping", &["managerId", "id"]),
        "agent_capabilities" => with_target(args, "probing", &["agentId", "id"]),
        // `harnessId` is the key the orchestrator actually sends; the others
        // are accepted so a rename upstream degrades to a bare verb, not a lie.
        "harness_select" => with_target(
            args,
            "selecting harness",
            &["harness", "harnessId", "kind", "id"],
        ),
        "agent_spawn" => with_target(args, "spawning agent", &["name", "templateId", "id"]),

        // Anything else: a readable verb from the name plus a scalar hint.
        other => generic(other, args),
    }
}

/// `deploy_managers({managers:[{name, objective}, …]})`.
fn deploy_summary(args: &Value) -> String {
    let managers = array(args, &["managers"]);
    match managers.len() {
        0 => "deploying a manager".into(),
        1 => {
            let m = &managers[0];
            let label = text(m, &["name", "id"]);
            let objective = text(m, &["objective", "goal"]);
            match (label, objective) {
                (Some(l), Some(o)) => format!("deploying manager {l} — {}", clip(&o)),
                (Some(l), None) => format!("deploying manager {l}"),
                (None, Some(o)) => format!("deploying a manager — {}", clip(&o)),
                (None, None) => "deploying a manager".into(),
            }
        }
        // Concurrency is the point of the call, so lead with the count and name
        // them; a reader wants to know the fan-out width at a glance.
        n => {
            let names: Vec<String> = managers
                .iter()
                .filter_map(|m| text(m, &["name", "id"]))
                .collect();
            if names.is_empty() {
                format!("deploying {n} managers")
            } else {
                format!("deploying {n} managers: {}", clip(&names.join(", ")))
            }
        }
    }
}

/// `delegate_tasks({tasks:[{instruction, agentId}, …]})`.
fn delegate_summary(args: &Value) -> String {
    let tasks = array(args, &["tasks"]);
    let target = text(args, &["agentId", "agent"])
        .or_else(|| tasks.first().and_then(|t| text(t, &["agentId", "agent"])));
    let head = match tasks.len() {
        0 | 1 => "delegating a task".to_string(),
        n => format!("delegating {n} tasks"),
    };
    let detail = if tasks.len() == 1 {
        tasks
            .first()
            .and_then(|t| text(t, &["instruction", "objective", "title"]))
    } else {
        None
    };
    match (target, detail) {
        (Some(t), Some(d)) => format!("{head} to {t} — {}", clip(&d)),
        (Some(t), None) => format!("{head} to {t}"),
        (None, Some(d)) => format!("{head} — {}", clip(&d)),
        (None, None) => head,
    }
}

/// `<verb> <target>`, falling back to the bare verb when no target is named.
fn with_target(args: &Value, verb: &str, keys: &[&str]) -> String {
    match text(args, keys) {
        Some(t) => format!("{verb} {}", clip(&t)),
        None => verb.to_string(),
    }
}

/// An unknown tool — typically a harness tool like `Bash` or `Read` rather than
/// one of the orchestrator's own.
///
/// Keeps the familiar `Name(argument)` shape: those names are already readable,
/// and rewriting them would churn the transcript for no gain. The only change
/// from the old renderer is what goes inside the parentheses — the first scalar
/// argument, never a nested object or array.
fn generic(name: &str, args: &Value) -> String {
    let hint = args
        .as_object()
        .into_iter()
        .flatten()
        .find_map(|(_, v)| scalar(v))
        .filter(|s| !s.trim().is_empty());
    match hint {
        Some(h) => format!("{name}({})", clip(&h)),
        None => name.to_string(),
    }
}

/// The first non-empty string at any of `keys`.
fn text(value: &Value, keys: &[&str]) -> Option<String> {
    let obj = value.as_object()?;
    keys.iter()
        .filter_map(|k| obj.get(*k))
        .filter_map(scalar)
        .find(|s| !s.trim().is_empty())
}

/// The array at the first matching key, or the value itself when it is already
/// one — providers sometimes send the bare list rather than the wrapper object.
fn array(value: &Value, keys: &[&str]) -> Vec<Value> {
    if let Some(items) = value.as_array() {
        return items.clone();
    }
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    keys.iter()
        .filter_map(|k| obj.get(*k))
        .find_map(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A scalar rendered as text. Objects and arrays yield `None` so no caller can
/// accidentally inline a JSON blob.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Collapse whitespace and clip to [`DETAIL_MAX`], marking an elision.
fn clip(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= DETAIL_MAX {
        return flat;
    }
    let head: String = flat.chars().take(DETAIL_MAX.saturating_sub(1)).collect();
    format!("{}…", head.trim_end())
}

#[cfg(test)]
#[path = "tool_call_tests.rs"]
mod tool_call_tests;
