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

#[cfg(test)]
mod tests;

/// Turn repository facts embedded in tool output into updated session facts.
///
/// A stable `worktree` report supplies the checkout and branch. GitHub CLI
/// commands print the pull-request URL they created or inspected. Either fact
/// is useful independently, and when one command prints both they travel in a
/// single update.
pub(super) fn workspace_event_from_output(
    output: &str,
    accept_pull_request: bool,
    line: i64,
    ts: i64,
    record_type: &str,
) -> Option<HarnessSemanticEvent> {
    let checkout = json_report(output).or_else(|| text_report(output));
    let pull_request = accept_pull_request
        .then(|| pull_request_url(output))
        .flatten();
    if checkout.is_none() && pull_request.is_none() {
        return None;
    }
    let mut payload = serde_json::Map::new();
    if let Some((cwd, branch)) = checkout {
        payload.insert("cwd".into(), Value::String(cwd));
        payload.insert("branch".into(), Value::String(branch));
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

/// Whether a completed shell call is a GitHub CLI PR create/view operation.
pub(super) fn is_pull_request_command(command: &str) -> bool {
    // Deliberately recognize only a direct invocation. Parsing chained shell
    // syntax here would also require correctly handling quotes and heredocs;
    // treating their contents as commands can attach a URL merely printed by
    // a fixture-building command.
    let mut words = command.split_whitespace();
    words.next() == Some("gh")
        && words.next() == Some("pr")
        && matches!(words.next(), Some("create" | "view"))
}

/// Find a GitHub pull-request URL in ordinary or JSON `gh` output.
///
/// Tokens are trimmed only at punctuation which cannot belong to a URL. The
/// `/pull/<number>` shape prevents an issue URL or an unrelated web link in a
/// command's output from being mistaken for session context.
fn pull_request_url(output: &str) -> Option<String> {
    const PREFIX: &str = "https://github.com/";
    output.match_indices(PREFIX).find_map(|(start, _)| {
        let token = output[start..]
            .split(|ch: char| {
                ch.is_whitespace() || matches!(ch, '"' | '\'' | ')' | ',' | '}' | ']')
            })
            .next()?;
        let url = token.strip_prefix(PREFIX)?;
        let (repo, number) = url.split_once("/pull/")?;
        let mut parts = repo.split('/');
        let owner = parts.next()?;
        let name = parts.next()?;
        if owner.is_empty() || name.is_empty() || parts.next().is_some() {
            return None;
        }
        let number = number.trim_end_matches('/');
        if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        Some(format!("{PREFIX}{owner}/{name}/pull/{number}"))
    })
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
