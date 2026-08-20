//! Recognizes repository context reported by Git and GitHub commands.
//!
//! A harness starts in the daemon's configured checkout, but may create and
//! continue in a linked worktree. The harness process itself retains its launch
//! cwd, so its init record is stale; the helper's completed report is the first
//! authoritative announcement of the new checkout.

use serde_json::Value;

use crate::harness_work::kinds;

use super::events::semantic;
use super::types::HarnessSemanticEvent;

mod pr;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use pr::{pull_request_command, pull_request_url, pull_request_url_from_json};
pub(crate) use types::{PendingPullRequestCall, PullRequestCommand};

/// Turn repository facts embedded in tool output into updated session facts.
///
/// A stable `worktree` report supplies the checkout and branch. GitHub CLI
/// commands print the pull-request URL they created or inspected. Either fact
/// is useful independently, and when one command prints both they travel in a
/// single update.
pub(crate) fn workspace_event_from_output(
    output: &str,
    pull_request_command: Option<PullRequestCommand>,
    workspace_cwd: Option<&str>,
    workspace_branch: Option<&str>,
    line: i64,
    ts: i64,
    record_type: &str,
) -> Option<HarnessSemanticEvent> {
    let checkout = worktree_checkout_from_output(output);
    let pull_request = pull_request_command.and_then(|command| match command {
        PullRequestCommand::Create => pull_request_url(output),
        PullRequestCommand::View => pull_request_url_from_json(output),
    });
    if checkout.is_none() && pull_request.is_none() {
        return None;
    }
    let mut payload = serde_json::Map::new();
    if let Some((cwd, branch)) = checkout {
        payload.insert("cwd".into(), Value::String(cwd));
        payload.insert("branch".into(), Value::String(branch));
    } else if pull_request.is_some() {
        if let Some(cwd) = workspace_cwd {
            payload.insert("cwd".into(), Value::String(cwd.to_string()));
        }
        if let Some(branch) = workspace_branch {
            payload.insert("branch".into(), Value::String(branch.to_string()));
        }
    }
    if let Some(url) = pull_request {
        payload.insert("pull_request".into(), Value::String(url));
    }
    Some(semantic(
        line,
        ts,
        record_type,
        kinds::SESSION_INFO,
        "agent",
        Value::Object(payload),
    ))
}

/// Read a stable worktree-helper report from command output.
///
/// Shared with transports that do not use the JSONL mapper (notably Codex
/// app-server), so every Codex execution path applies the same signature checks
/// before changing a session's runtime directory.
pub(crate) fn worktree_checkout_from_output(output: &str) -> Option<(String, String)> {
    json_report(output).or_else(|| text_report(output))
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
