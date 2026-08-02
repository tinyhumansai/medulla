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

/// Read the `--json` report, allowing harmless log text before the object.
fn json_report(output: &str) -> Option<(String, String)> {
    let start = output.find('{')?;
    let value: Value = serde_json::from_str(&output[start..]).ok()?;
    if value.get("status").and_then(Value::as_str) != Some("ready") {
        return None;
    }
    report_fields(&value)
}

/// Read the default stable `[PASS] WORKTREE_READY` report.
fn text_report(output: &str) -> Option<(String, String)> {
    if !output
        .lines()
        .any(|line| line.trim() == "[PASS] WORKTREE_READY")
    {
        return None;
    }
    let field = |name: &str| {
        output.lines().find_map(|line| {
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
