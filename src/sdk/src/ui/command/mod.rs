//! Slash-command parsing and the `/copy` transcript helper.
//!
//! [`parse`] classifies a raw composer line into a [`SlashCommand`] without
//! touching any UI state; the front end matches on the result to run the side
//! effect. [`copy_text`] renders the transcript for a [`CopyScope`]. Keeping the
//! parse pure makes the command surface testable and reusable across front ends.

mod types;

#[cfg(test)]
mod tests;

pub use types::{CopyScope, SlashCommand};

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
/// matched case-insensitively; free-text arguments (`/fork`, `/memory`) preserve
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
        "quit" | "q" => SlashCommand::Quit,
        "new" => SlashCommand::NewSession,
        "fork" => SlashCommand::Fork(non_empty(arg)),
        "resume" => SlashCommand::Resume,
        "abort" => SlashCommand::Abort,
        "clear" => SlashCommand::ClearView,
        "help" => SlashCommand::Help,
        "config" => SlashCommand::Config,
        "settings" | "theme" => SlashCommand::Settings,
        "usage" => SlashCommand::Usage,
        "memory" | "mem" => SlashCommand::Memory(non_empty(arg)),
        "lesson" => match crate::lessons::parse_lesson_spec(arg) {
            Ok(lesson) => SlashCommand::Lesson {
                trigger: lesson.trigger,
                rule: lesson.rule,
            },
            Err(_) => SlashCommand::BadUsage("Usage: /lesson <trigger> -> <rule>"),
        },
        "feedback" | "fb" => SlashCommand::Feedback,
        "mouse" => SlashCommand::ToggleMouse,
        "copy" => match flag.as_str() {
            "" | "all" => SlashCommand::Copy(CopyScope::All),
            "last" => SlashCommand::Copy(CopyScope::Last),
            _ => SlashCommand::BadUsage("Usage: /copy [all|last]"),
        },
        "async" => match flag.as_str() {
            "" => SlashCommand::Async(None),
            "on" => SlashCommand::Async(Some(true)),
            "off" => SlashCommand::Async(Some(false)),
            _ => SlashCommand::BadUsage("Usage: /async [on|off]"),
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

/// `Some(trimmed)` when `s` has non-whitespace content, else `None`.
fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}
