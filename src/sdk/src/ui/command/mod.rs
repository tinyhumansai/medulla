//! Slash-command parsing, the command catalog, and the `/copy` transcript
//! helper.
//!
//! [`parse`] classifies a raw composer line into a [`SlashCommand`] without
//! touching any UI state; the front end matches on the result to run the side
//! effect. [`copy_text`] renders the transcript for a [`CopyScope`]. Keeping the
//! parse pure makes the command surface testable and reusable across front ends.

mod catalog;
mod types;

#[cfg(test)]
mod tests;

pub use catalog::{lookup, suggestions, CommandSpec, COMMANDS};
pub use types::{CopyScope, SlashCommand};

use crate::tinyplace::HarnessProvider;
use crate::ui::events::{chat_transcript, last_assistant_message, EventEnvelope};

impl SlashCommand {
    /// Parse a composer line into a command; see [`parse`] (module function) for
    /// the full contract. `None` means "not a slash command".
    pub fn parse(input: &str) -> Option<SlashCommand> {
        parse(input)
    }
}

/// Parse a composer line into a [`SlashCommand`].
///
/// Returns `None` when `input` is not a slash command (no leading `/` after
/// trimming) so the caller can treat it as a normal prompt. The command token is
/// matched case-insensitively; free-text arguments (`/memory`) preserve
/// their original case, while flag arguments (`/copy`, `/async`) are matched
/// case-insensitively. Unrecognized commands map to [`SlashCommand::Unknown`] and
/// invalid arguments to [`SlashCommand::BadUsage`], so no input is silently
/// dropped.
pub fn parse(input: &str) -> Option<SlashCommand> {
    let rest = input.trim().strip_prefix('/')?.trim();
    let (cmd_raw, arg) = match rest.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (rest, ""),
    };
    let cmd = cmd_raw.to_lowercase();
    let flag = arg.to_lowercase();
    Some(match cmd.as_str() {
        "quit" | "q" | "exit" => SlashCommand::Quit,
        "new" => SlashCommand::NewSession,
        "resume" => SlashCommand::Resume,
        "harness" => parse_harness(arg),
        "takecontrol" | "take" => SlashCommand::TakeControl,
        "handoff" | "hand" => SlashCommand::HandOff,
        "abort" => SlashCommand::Abort,
        "clear" => SlashCommand::ClearView,
        "help" => SlashCommand::Help,
        "config" => SlashCommand::Config,
        "settings" | "theme" => SlashCommand::Settings,
        "usage" => SlashCommand::Usage,
        "memory" | "mem" => SlashCommand::Memory(non_empty(arg)),
        "mouse" => SlashCommand::ToggleMouse,
        "copy" => match flag.as_str() {
            "" | "all" => SlashCommand::Copy(CopyScope::All),
            "last" => SlashCommand::Copy(CopyScope::Last),
            _ => SlashCommand::BadUsage("Usage: /copy [all|last]"),
        },
        _ => SlashCommand::Unknown(input.trim().to_string()),
    })
}

/// The text a `/copy` command should place on the clipboard for `scope`.
///
/// [`CopyScope::Last`] yields the most recent assistant reply (empty when there
/// is none); [`CopyScope::All`] yields the full chat transcript.
pub fn copy_text(events: &[EventEnvelope], scope: CopyScope) -> String {
    match scope {
        CopyScope::Last => last_assistant_message(events).unwrap_or_default(),
        CopyScope::All => chat_transcript(events),
    }
}

/// Parse the argument tail of `/harness` into its provider and path.
///
/// The shapes are `/harness`, `/harness <provider>`, and
/// `/harness <provider> <path>`. A named provider is validated here, against the
/// same [`HarnessProvider::from_wire`] the wire uses, because "claud" should say
/// so rather than silently starting the default harness — that failure is
/// invisible until the wrong CLI is already running in the operator's workspace.
///
/// The path is not validated: only the front end knows the active workspace, and
/// a bad path produces a far better message at spawn time than at parse time.
fn parse_harness(arg: &str) -> SlashCommand {
    const USAGE: &str = "Usage: /harness [claude|codex|opencode] [path]";
    let arg = arg.trim();
    if arg.is_empty() {
        return SlashCommand::NewHarness {
            provider: None,
            path: None,
        };
    }
    let (provider_raw, path) = match arg.split_once(char::is_whitespace) {
        Some((p, rest)) => (p, non_empty(rest)),
        None => (arg, None),
    };
    let provider = provider_raw.to_lowercase();
    if HarnessProvider::from_wire(&provider).is_none() {
        return SlashCommand::BadUsage(USAGE);
    }
    SlashCommand::NewHarness {
        provider: Some(provider),
        path,
    }
}

/// `Some(trimmed)` when `s` has non-whitespace content, else `None`.
fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}
