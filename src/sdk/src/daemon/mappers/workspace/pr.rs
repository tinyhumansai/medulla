//! Recognizes safe GitHub CLI PR commands and parses their authoritative URLs.

use serde_json::Value;

use super::PullRequestCommand;

/// Whether a completed shell call is a GitHub CLI PR create/view operation.
pub(crate) fn pull_request_command(
    command: &str,
    workspace_cwd: Option<&str>,
    gh_repo_is_set: bool,
) -> Option<PullRequestCommand> {
    if gh_repo_is_set {
        return None;
    }
    let inner = shell_inner_command(command);
    let command = inner.as_deref().unwrap_or(command).trim();
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
    let unquoted_match = workspace_cwd
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-'))
        .then(|| command.strip_prefix(&unquoted))
        .flatten();
    unquoted_match.or_else(|| command.strip_prefix(&quoted))
}

/// Recognize the argv prefix of a direct GitHub CLI PR operation.
fn direct_pull_request_command(command: &str) -> Option<PullRequestCommand> {
    if has_executable_shell_operator(command) {
        return None;
    }
    let words = shell_words(command)?;
    let mut words = words.iter().map(String::as_str);
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
            || has_short_option(argument, 'R')
    }) {
        return None;
    }
    match operation {
        "create" if !has_explicit_head(&arguments) && !has_dry_run(&arguments) => {
            Some(PullRequestCommand::Create)
        }
        "view" if arguments == ["--json", "url"] || arguments == ["--json=url"] => {
            Some(PullRequestCommand::View)
        }
        _ => None,
    }
}

/// Detect executable operators while treating single-quoted characters as data.
fn has_executable_shell_operator(command: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => escaped = true,
            (Some('"'), '$' | '`') => return true,
            (Some('"'), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (
                None,
                '\n' | '\r' | ';' | '|' | '&' | '$' | '`' | '<' | '>' | '{' | '}' | '*' | '?' | '['
                | ']',
            ) => return true,
            (None, _) => {}
            _ => unreachable!(),
        }
    }
    false
}

/// Tokenize the conservative direct-command subset with shell quote semantics.
fn shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => word.push(ch),
            (_, '\\') => escaped = true,
            (Some('"'), _) => word.push(ch),
            (None, '\'' | '"') => quote = Some(ch),
            (None, ch) if ch.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            (None, _) => word.push(ch),
            _ => unreachable!(),
        }
    }
    if escaped || quote.is_some() {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

/// Whether PR creation explicitly names a branch other than the current one.
fn has_explicit_head(arguments: &[&str]) -> bool {
    arguments.iter().any(|argument| {
        matches!(*argument, "--head" | "-H")
            || argument.starts_with("--head=")
            || has_short_option(argument, 'H')
    })
}

/// Whether a pflag shorthand group contains `sought` before any joined value.
fn has_short_option(argument: &str, sought: char) -> bool {
    let Some(group) = argument
        .strip_prefix('-')
        .filter(|group| !group.starts_with('-'))
    else {
        return false;
    };
    for shorthand in group.chars() {
        if shorthand == sought {
            return true;
        }
        // Boolean shorthands may be bundled. Every other known create
        // shorthand consumes the remainder as its joined value.
        if !matches!(shorthand, 'd' | 'e' | 'f' | 'w') {
            return false;
        }
    }
    false
}

/// Whether PR creation is only simulating its result.
fn has_dry_run(arguments: &[&str]) -> bool {
    arguments
        .iter()
        .any(|argument| *argument == "--dry-run" || argument.starts_with("--dry-run="))
}

/// Read only the structured `url` property from `gh pr view --json url`.
pub(crate) fn pull_request_url_from_json(output: &str) -> Option<String> {
    output.match_indices('{').find_map(|(start, _)| {
        let value = serde_json::Deserializer::from_str(&output[start..])
            .into_iter::<Value>()
            .next()?
            .ok()?;
        pull_request_url(value.get("url")?.as_str()?)
    })
}

/// Unwrap the single-quoted `shell -lc '…'` shape recorded by Codex.
fn shell_inner_command(command: &str) -> Option<String> {
    let command = command.trim();
    let rest = ["/bin/zsh", "zsh", "/bin/bash", "bash", "/bin/sh", "sh"]
        .iter()
        .find_map(|shell| command.strip_prefix(shell))?
        .trim_start();
    let quoted = rest.strip_prefix("-lc")?.trim_start();
    decode_single_quoted_word(quoted)
}

/// Decode one shell word composed of single-quoted spans and escaped quotes.
///
/// Codex records `shell -lc` payloads as a single shell-quoted argument. A
/// literal apostrophe is represented by closing the span, writing `\'`, and
/// reopening it (`'Don'\''t'`). Reject every other unquoted form so decoding
/// cannot broaden the accepted direct-command subset.
fn decode_single_quoted_word(word: &str) -> Option<String> {
    let mut decoded = String::new();
    let mut chars = word.chars().peekable();
    let mut quoted = false;
    let mut saw_quote = false;
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '\'' {
                quoted = false;
            } else {
                decoded.push(ch);
            }
            continue;
        }
        match ch {
            '\'' => {
                quoted = true;
                saw_quote = true;
            }
            '\\' if chars.next() == Some('\'') => decoded.push('\''),
            _ => return None,
        }
    }
    (saw_quote && !quoted).then_some(decoded)
}

/// Find a GitHub pull-request URL in ordinary or JSON `gh` output.
pub(crate) fn pull_request_url(output: &str) -> Option<String> {
    const PREFIX: &str = "https://github.com/";
    output.match_indices(PREFIX).find_map(|(start, _)| {
        let token = output[start..]
            .split(|ch: char| {
                ch.is_whitespace() || matches!(ch, '"' | '\'' | ')' | ',' | '}' | ']')
            })
            .next()?;
        let url = token.strip_prefix(PREFIX)?;
        let (repo, number) = url.rsplit_once("/pull/")?;
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
