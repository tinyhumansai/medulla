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

/// GitHub CLI operation whose output can authoritatively identify this PR.
#[derive(Clone, Copy)]
pub(super) enum PullRequestCommand {
    /// `gh pr create` prints the newly created PR URL.
    Create,
    /// `gh pr view --json url` returns a structured URL property.
    View,
}

/// Turn repository facts embedded in tool output into updated session facts.
///
/// A stable `worktree` report supplies the checkout and branch. GitHub CLI
/// commands print the pull-request URL they created or inspected. Either fact
/// is useful independently, and when one command prints both they travel in a
/// single update.
pub(super) fn workspace_event_from_output(
    output: &str,
    pull_request_command: Option<PullRequestCommand>,
    line: i64,
    ts: i64,
    record_type: &str,
) -> Option<HarnessSemanticEvent> {
    let checkout = json_report(output).or_else(|| text_report(output));
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
pub(super) fn pull_request_command(
    command: &str,
    workspace_cwd: Option<&str>,
) -> Option<PullRequestCommand> {
    // Deliberately recognize only a direct invocation. Parsing chained shell
    // syntax here would also require correctly handling quotes and heredocs;
    // treating their contents as commands can attach a URL merely printed by
    // a fixture-building command.
    let command = shell_inner_command(command).unwrap_or(command).trim();
    match workspace_cwd {
        Some(workspace_cwd) => {
            direct_pull_request_command(command_after_cd(command, workspace_cwd)?)
        }
        None => direct_pull_request_command(command),
    }
}

/// Accept the central-worktree `cd <reported cwd> && gh ...` form only.
fn command_after_cd<'a>(command: &'a str, workspace_cwd: &str) -> Option<&'a str> {
    let unquoted = format!("cd {workspace_cwd} && ");
    let quoted = format!("cd '{}' && ", workspace_cwd.replace('\'', "'\\''"));
    command
        .strip_prefix(&unquoted)
        .or_else(|| command.strip_prefix(&quoted))
}

/// Recognize the argv prefix of a direct GitHub CLI PR operation.
fn direct_pull_request_command(command: &str) -> Option<PullRequestCommand> {
    if command
        .chars()
        .any(|ch| matches!(ch, '\n' | '\r' | ';' | '|' | '&' | '$' | '`' | '<' | '>'))
    {
        return None;
    }
    let mut words = command.split_whitespace();
    if words.next() != Some("gh") || words.next() != Some("pr") {
        return None;
    }
    let Some(operation @ ("create" | "view")) = words.next() else {
        return None;
    };
    let arguments = words.collect::<Vec<_>>();
    if arguments.iter().any(|argument| {
        matches!(*argument, "--repo" | "-R")
            || argument.starts_with("--repo=")
            || (argument.starts_with("-R") && argument.len() > 2)
    }) {
        return None;
    }
    match operation {
        "create" if !has_explicit_head(&arguments) => Some(PullRequestCommand::Create),
        "view" if arguments == ["--json", "url"] => Some(PullRequestCommand::View),
        _ => None,
    }
}

/// Whether PR creation explicitly names a branch other than the current one.
fn has_explicit_head(arguments: &[&str]) -> bool {
    arguments.iter().any(|argument| {
        matches!(*argument, "--head" | "-H")
            || argument.starts_with("--head=")
            || (argument.starts_with("-H") && argument.len() > 2)
    })
}

/// Read only the structured `url` property from `gh pr view --json url`.
fn pull_request_url_from_json(output: &str) -> Option<String> {
    output.match_indices('{').find_map(|(start, _)| {
        let value = serde_json::Deserializer::from_str(&output[start..])
            .into_iter::<Value>()
            .next()?
            .ok()?;
        pull_request_url(value.get("url")?.as_str()?)
    })
}

/// Unwrap the single-quoted `shell -lc '…'` shape recorded by Codex.
///
/// This is intentionally narrower than shell parsing: accepting only the
/// captured launcher shape preserves quoting boundaries and never examines a
/// heredoc or a quoted fixture body as executable syntax.
fn shell_inner_command(command: &str) -> Option<&str> {
    let command = command.trim();
    let rest = ["/bin/zsh", "zsh", "/bin/bash", "bash", "/bin/sh", "sh"]
        .iter()
        .find_map(|shell| command.strip_prefix(shell))?
        .trim_start();
    let quoted = rest.strip_prefix("-lc")?.trim_start();
    quoted.strip_prefix('\'')?.strip_suffix('\'')
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
