//! Attention cues that come from the harness's own lifecycle reports.
//!
//! [`super::detect`] reads the screen, which is the right surface for everything
//! a harness *says*. It is the wrong surface for the one thing a harness says
//! through its hooks rather than its terminal: when Claude Code is stopped on
//! the operator — a tool use awaiting approval, an MCP elicitation form awaiting
//! input, a background agent waiting on you — it raises a `Notification`
//! lifecycle hook, and that report is the harness saying so itself, from the one
//! channel that does not reword its prompts, wrap its menus, or hide behind a
//! full-screen TUI the scraper has to survive.
//!
//! Medulla's built-in Notification hook is installed with a matcher that names
//! exactly those stopping types (see [`medulla::harness_hooks::builtin`]). Claude
//! Code fires `Notification` for informational events as well — above all
//! `idle_prompt`, every time a finished turn returns to the prompt — but that is
//! the ordinary resting composer this module's
//! [`AttentionKind::Completed`](super::types::AttentionKind::Completed) exists
//! to keep out of the "waiting on you" count, so the built-in does not report
//! those at all. A `Notification` in the hook log therefore *is* a wait.
//!
//! The screen scraper is still the primary cue: a named permission menu it can
//! read is more specific than a generic "waiting". This is the *fallback* that
//! fills in when the screen has nothing — a harness stopped on a prompt the
//! markers do not recognise still stopped, and an elicitation form or a
//! background agent waiting paints nothing the scraper can name at all.
//!
//! Only Claude Code raises `Notification` (see
//! [`medulla::harness_hooks::HookEvent::supported_by`]), so in practice this cue
//! is Claude's alone. Codex never reports it, so [`hook_attention`] returns
//! `None` for a codex session without any special-casing — the last event it
//! reports is never `Notification`.

use medulla::harness_hooks::{HookEvent, HookEventLog};
use medulla::protocol::HarnessProvider;

use super::types::AttentionKind;

/// The kind and label of the cue a harness's last lifecycle report warrants, if
/// any.
///
/// Returns `Some` only when the session's most recent hook report was
/// [`HookEvent::Notification`] — i.e. the harness itself said it is waiting on
/// the operator — and the session is not mid-turn. The `working` gate is what
/// keeps a stale report blinking: `Notification` lags the screen by one hook
/// dispatch, so a harness that resumed after the operator answered would
/// otherwise keep showing "waiting for you" until the next `PostToolUse`
/// arrived. The screen's own working footer is the earlier truth, so it vetoes.
///
/// Mirrors [`super::detect`] and [`super::bell_cue`] in returning the
/// `(AttentionKind, String)` pair and leaving the `since` stamp to the poller,
/// so the same held-cue preservation keeps a cue that holds stable across
/// repaints.
pub fn hook_attention(
    provider: HarnessProvider,
    grant: Option<&str>,
    working: bool,
    log: &HookEventLog,
) -> Option<(AttentionKind, String)> {
    if working {
        return None;
    }
    let grant = grant?;
    if log.last_event(grant) != Some(HookEvent::Notification) {
        return None;
    }
    Some((
        AttentionKind::Approval,
        format!("{} is waiting for you", provider.as_str()),
    ))
}

/// Whether the most recent lifecycle report says a turn is active.
///
/// `Some(true)` covers every point inside a turn, from prompt submission through
/// tools, subagents, and compaction. `Some(false)` covers an idle, waiting, or
/// ended session. `None` means this session has not reported enough lifecycle
/// information, so the caller should fall back to the terminal screen.
///
/// The hook is the stable half of working-state detection for Claude Code. Its
/// progress wording and spinner glyphs change frequently, while these event
/// names are the API contract used to run hooks in the first place.
pub fn hook_working(grant: Option<&str>, log: &HookEventLog) -> Option<bool> {
    let event = log.last_event(grant?)?;
    Some(matches!(
        event,
        HookEvent::UserPromptSubmit
            | HookEvent::PreToolUse
            | HookEvent::PostToolUse
            | HookEvent::SubagentStart
            | HookEvent::SubagentStop
            | HookEvent::PreCompact
            | HookEvent::PostCompact
    ))
}
