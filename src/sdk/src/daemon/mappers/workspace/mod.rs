//! Recognizes the stable report printed by the repository `worktree` helper.
//!
//! A harness starts in the daemon's configured checkout, but may create and
//! continue in a linked worktree. The harness process itself retains its launch
//! cwd, so its init record is stale; the helper's completed report is the first
//! authoritative announcement of the new checkout.

use serde_json::Value;

use crate::harness_work::kinds;

use super::events::semantic;
use super::types::HarnessSemanticEvent;

#[cfg(test)]
mod tests;

/// Turn a successful `worktree` report embedded in tool output into updated
/// session facts. Unrelated output and incomplete reports produce no event.
pub(super) fn workspace_event_from_output(
    output: &str,
    line: i64,
    ts: i64,
    record_type: &str,
) -> Option<HarnessSemanticEvent> {
    let (cwd, branch) = json_report(output).or_else(|| text_report(output))?;
    Some(semantic(
        line,
        ts,
        record_type,
        kinds::SESSION_INFO,
        "agent",
        serde_json::json!({ "cwd": cwd, "branch": branch }),
    ))
}

/// Read the `--json` report, allowing command output around the object.
fn json_report(output: &str) -> Option<(String, String)> {
    output.match_indices('{').find_map(|(start, _)| {
        let value = serde_json::Deserializer::from_str(&output[start..])
            .into_iter::<Value>()
            .next()?
            .ok()?;
        has_worktree_signature(&value)
            .then(|| report_fields(&value))
            .flatten()
    })
}

/// Verify fields unique to the helper's stable JSON contract before trusting
/// generic names such as `path` and `branch` as session facts.
fn has_worktree_signature(value: &Value) -> bool {
    value.get("status").and_then(Value::as_str) == Some("ready")
        && value.get("repository").and_then(Value::as_str).is_some()
        && value.get("head").and_then(Value::as_str).is_some()
        && value.get("headShort").and_then(Value::as_str).is_some()
        && value.get("created").and_then(Value::as_bool).is_some()
        && value.get("nextCommand").and_then(Value::as_str).is_some()
        && value.pointer("/submodules/state").and_then(Value::as_str)
            == Some("initialized_recursive")
        && value
            .pointer("/submodules/count")
            .and_then(Value::as_u64)
            .is_some()
}

/// Read the default stable `[PASS] WORKTREE_READY` report.
fn text_report(output: &str) -> Option<(String, String)> {
    let report = output
        .lines()
        .skip_while(|line| line.trim() != "[PASS] WORKTREE_READY")
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .collect::<Vec<_>>();
    let field = |name: &str| {
        report.iter().find_map(|line| {
            line.trim()
                .strip_prefix(name)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
    };
    Some((field("path:")?, field("branch:")?))
}

/// Extract the required checkout fields from a JSON report.
fn report_fields(value: &Value) -> Option<(String, String)> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    Some((text("path")?, text("branch")?))
}
