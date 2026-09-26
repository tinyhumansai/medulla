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
    Some(hook_event_marks_active_turn(event))
}

/// Whether `event`, as a session's most recent lifecycle report, means a turn
/// is active.
///
/// Factored out of [`hook_working`] so a caller that already holds one
/// [`HookReport`](medulla::harness_hooks::HookReport) snapshot — event and
/// `at_ms` together — can classify it without a second, separately-timed read
/// of the log. Two reads of "the last report" a poll apart can name different
/// reports if one is recorded in between; deriving both the active/idle
/// verdict and the report's age from the *same* snapshot is what keeps a
/// classification from mixing an old report's age with a newer report's
/// event.
pub fn hook_event_marks_active_turn(event: HookEvent) -> bool {
    matches!(
        event,
        HookEvent::UserPromptSubmit
            | HookEvent::PreToolUse
            | HookEvent::PostToolUse
            | HookEvent::SubagentStart
            | HookEvent::SubagentStop
            | HookEvent::PreCompact
            | HookEvent::PostCompact
    )
}

/// Whether `event` opens an operation of unbounded duration rather than
/// marking a boundary after which visible activity is expected imminently.
///
/// `PreToolUse`, `SubagentStart`, and `PreCompact` say an operation *started*
/// and has not yet returned — a slow build under `Bash`, a long-running
/// subagent, a large compaction — and there is no legitimate upper bound on
/// how long that can take while the harness is genuinely still working. Their
/// matching `Post`/`Stop` event is what closes them.
///
/// Every other active event (`UserPromptSubmit`, `PostToolUse`,
/// `SubagentStop`, `PostCompact`) instead marks a moment *just after* which the
/// harness should paint something visible again within moments — the model
/// resuming generation, a fresh spinner — so a report stuck there for a long
/// time with nothing on screen to corroborate it is what a dropped follow-up
/// report looks like. Only that second group is a candidate for the staleness
/// grace in [`super::super::manager::attention::working_state`]; this function
/// is what tells the caller which group a report falls into.
pub fn hook_report_is_open_ended(event: HookEvent) -> bool {
    matches!(
        event,
        HookEvent::PreToolUse | HookEvent::SubagentStart | HookEvent::PreCompact
    )
}
