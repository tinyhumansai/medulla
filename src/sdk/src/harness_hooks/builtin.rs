//! The hooks Medulla installs into every harness it launches, without being
//! asked.
//!
//! Everything else in this module is a translator for hooks an *operator*
//! declared. These are Medulla's own, and they exist because a harness on a pty
//! is otherwise opaque: the only thing Medulla can see of a Claude Code or Codex
//! session it started is the bytes it paints, so "is this session waiting for
//! me?" and "did that turn finish?" were answered by pattern-matching a screen.
//! A lifecycle hook answers both as fact, from the harness itself.
//!
//! # What they run
//!
//! Every built-in runs the same command — `medulla hook <Event>` — against the
//! Medulla binary that launched the harness. The shim reads the harness's native
//! payload on stdin and forwards it to the control socket the spawn was already
//! handed (see [`crate::control_socket`]), authenticating with that session's
//! own grant. It writes nothing to stdout and always exits zero: a hook that
//! failed loudly would interrupt the operator's session to report that Medulla's
//! own bookkeeping had a bad day.
//!
//! A session spawned without a grant — fleet tools off, a remote worker, a
//! harness the operator started themselves — runs the same command and it exits
//! immediately, having found nothing to report to.
//!
//! # Which events
//!
//! [`REPORTED_EVENTS`] is deliberately the *observational* half of the
//! vocabulary. No `PreToolUse` and no `PermissionRequest`: both can deny a call,
//! and a built-in Medulla installs everywhere must not be able to change what a
//! session does. Reporting hooks can only be late, never wrong.

use crate::protocol::HarnessProvider;

use super::types::{HookEvent, HookHandler, HookSpec};

/// The lifecycle events Medulla reports on by default.
///
/// `Notification` is Claude Code's alone (Codex does not raise it), so its
/// built-in is restricted to Claude rather than left to be dropped — a
/// per-spawn "Codex does not raise Notification" note on every Codex launch is
/// noise about a hook the operator never declared.
pub const REPORTED_EVENTS: [HookEvent; 8] = [
    HookEvent::SessionStart,
    HookEvent::UserPromptSubmit,
    HookEvent::PostToolUse,
    HookEvent::Stop,
    HookEvent::SubagentStop,
    HookEvent::PreCompact,
    HookEvent::SessionEnd,
    HookEvent::Notification,
];

/// Seconds a report may take before the harness abandons it.
///
/// Short on purpose. The shim connects to a local unix socket and writes one
/// frame; anything slower than this means Medulla is gone or wedged, and the
/// right answer is to drop the report rather than hold up the session.
const REPORT_TIMEOUT_SECS: u64 = 5;

/// [`REPORT_TIMEOUT_SECS`] for `SessionEnd` alone.
///
/// Codex's hook config refuses a `SessionEnd` handler whose declared timeout
/// exceeds three seconds (its own hook docs cap it there), so serializing the
/// ordinary five-second budget onto that one event would hand a default Codex
/// launch an unsupported hook definition for the very event it is supposed to
/// report on exit. Capping it costs nothing here: the shim's own internal
/// budget ([`REPORT_TIMEOUT_SECS`]'s sibling `BUDGET` in the `medulla-tui`
/// crate's `commands::hook`) is already three seconds, under both caps.
const SESSION_END_TIMEOUT_SECS: u64 = 3;

/// The declared timeout for `event`'s built-in, in seconds.
fn timeout_for(event: HookEvent) -> u64 {
    match event {
        HookEvent::SessionEnd => SESSION_END_TIMEOUT_SECS,
        _ => REPORT_TIMEOUT_SECS,
    }
}

/// The `medulla hook` subcommand name, shared with the CLI that implements it.
pub const HOOK_SUBCOMMAND: &str = "hook";

/// The built-in hooks, bound to `bin` — the Medulla executable the harness's
/// hooks should call back into.
///
/// Callers normally pass [`medulla_bin`]. It is a parameter rather than a
/// lookup so the set is a pure function of its input, testable without a real
/// installation.
pub fn hooks(bin: &str) -> Vec<HookSpec> {
    REPORTED_EVENTS
        .iter()
        .map(|event| spec(bin, *event))
        .collect()
}

/// One built-in, for `event`.
fn spec(bin: &str, event: HookEvent) -> HookSpec {
    HookSpec {
        event,
        matcher: "*".to_string(),
        handler: HookHandler::Command {
            command: format!("{} {HOOK_SUBCOMMAND} {}", shell_quote(bin), event.as_str()),
            timeout: Some(timeout_for(event)),
        },
        // Empty for everything both harnesses raise; see `REPORTED_EVENTS`.
        harnesses: match event {
            HookEvent::Notification => vec![HarnessProvider::Claude],
            _ => Vec::new(),
        },
        label: Some(format!("Medulla · report {}", event.as_str())),
        builtin: true,
    }
}

/// The Medulla executable a spawned harness should call back into.
///
/// The *running* binary, not whatever `medulla` resolves to on the child's
/// PATH: a harness launched from a development build must report to that build,
/// and an operator with no Medulla on their PATH at all is the ordinary case for
/// a downloaded release. Falls back to the bare command name when the running
/// executable cannot be resolved, which leaves the hook working wherever Medulla
/// is installed normally.
pub fn medulla_bin() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_else(|| "medulla".to_string())
}

/// Quote `value` — always [`medulla_bin`]'s own path, never operator input —
/// for the shell that runs a hook command on this platform.
///
/// A thin wrapper over [`shell_quote_for`] so production code always quotes
/// for the platform it is actually running on, while tests can exercise both
/// rules regardless of which platform runs them.
fn shell_quote(value: &str) -> String {
    shell_quote_for(value, cfg!(windows))
}

/// Quote `value` for a POSIX shell, or for `cmd.exe`/PowerShell when
/// `windows` is set.
///
/// An installation path containing a space is not exotic — `/Applications/…`
/// and `C:\Program Files\…` both produce one — so both rules exist for the
/// same reason: the built-in's own command line must survive being split by
/// the shell that reads it. POSIX closes, escapes, and reopens an embedded
/// single quote, the one form those shells accept inside a quoted word;
/// Windows instead doubles an embedded double quote, which both `cmd.exe` and
/// PowerShell accept inside a double-quoted argument. This only ever has to
/// survive quoting the running binary's own path — never an operator's hook
/// command, which is never passed through here — so it does not attempt
/// Codex's separate `commandWindows` override or Claude's Git-Bash-vs-
/// PowerShell distinction for arbitrary shell text, neither of which this
/// crate can verify without a real Windows harness to run against.
fn shell_quote_for(value: &str, windows: bool) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        return value.to_string();
    }
    if windows {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
#[path = "builtin_tests.rs"]
mod tests;
