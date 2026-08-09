//! Attention cues that come from the harness's own lifecycle reports.
//!
//! [`super::detect`] reads the screen, which is the right surface for everything
//! a harness *says*. It is the wrong surface for the one thing a harness says
//! through its hooks rather than its terminal: Claude Code raises a `Notification`
//! lifecycle hook when it is waiting on the operator — a permission prompt it is
//! holding on, or an idle "waiting for your input" — and that report is the
//! harness saying so itself, from the one channel that does not reword its
//! prompts, wrap its menus, or hide behind a full-screen TUI the scraper has to
//! survive.
//!
//! The screen scraper is still the primary cue: a named permission menu it can
//! read is more specific than a generic "waiting". This is the *fallback* that
//! fills in when the screen has nothing — a harness stopped on a prompt the
//! markers do not recognise still stopped, and a harness waiting for input
//! paints nothing the scraper can name at all.
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
