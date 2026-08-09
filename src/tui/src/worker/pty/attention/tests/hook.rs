//! Unit tests for the hook-derived attention cue.
//!
//! These cover [`super::super::hook::hook_attention`]: the cue a session derives
//! from a `Notification` lifecycle report rather than from its screen. The
//! helper is a pure function of the hook log, so the tests drive it with a
//! synthetic [`HookEventLog`] and no live pty.

use medulla::harness_hooks::{HookEvent, HookEventLog, HookReport};
use medulla::protocol::HarnessProvider;

use super::super::hook::hook_attention;
use super::super::types::AttentionKind;

/// The grant key a session's MCP fleet hook reports are filed under.
const GRANT: &str = "grant-session-1";

/// A log whose last report for `GRANT` is `event`.
fn log_with_last(event: HookEvent) -> HookEventLog {
    let log = HookEventLog::new();
    // An earlier, different event the `event` must supersede as "last".
    log.record(HookReport::new(GRANT, HookEvent::PostToolUse));
    log.record(HookReport::new(GRANT, event));
    log
}

#[test]
fn notification_raises_an_approval_cue_for_claude() {
    let log = log_with_last(HookEvent::Notification);
    let (kind, what) = hook_attention(HarnessProvider::Claude, Some(GRANT), false, &log)
        .expect("a claude session that reported Notification waits");
    assert_eq!(kind, AttentionKind::Approval);
    assert_eq!(what, "claude is waiting for you");
}

#[test]
fn a_session_working_through_its_report_does_not_wait() {
    // The operator answered; the screen says the harness resumed, but the hook
    // log has not caught up. The working veto keeps the stale report from
    // blinking a row that is already busy again.
    let log = log_with_last(HookEvent::Notification);
    assert_eq!(
        hook_attention(HarnessProvider::Claude, Some(GRANT), true, &log),
        None
    );
}

#[test]
fn a_stop_report_means_idle_not_waiting() {
    let log = log_with_last(HookEvent::Stop);
    assert_eq!(
        hook_attention(HarnessProvider::Claude, Some(GRANT), false, &log),
        None
    );
}

#[test]
fn a_mid_turn_report_means_working_not_waiting() {
    let log = log_with_last(HookEvent::PostToolUse);
    assert_eq!(
        hook_attention(HarnessProvider::Claude, Some(GRANT), false, &log),
        None
    );
}

#[test]
fn a_session_with_no_grant_reports_nothing() {
    let log = log_with_last(HookEvent::Notification);
    assert_eq!(
        hook_attention(HarnessProvider::Claude, None, false, &log),
        None
    );
}

#[test]
fn a_session_the_log_has_no_report_for_reports_nothing() {
    let log = log_with_last(HookEvent::Notification);
    assert_eq!(
        hook_attention(
            HarnessProvider::Claude,
            Some("a-different-grant"),
            false,
            &log
        ),
        None
    );
}

#[test]
fn an_empty_log_reports_nothing() {
    let log = HookEventLog::new();
    assert_eq!(
        hook_attention(HarnessProvider::Claude, Some(GRANT), false, &log),
        None
    );
}

#[test]
fn a_newer_non_notification_supersedes_an_older_notification() {
    // Notification, then the operator answered and a tool ran: the last event
    // is PostToolUse now, so the wait is over even though a Notification sits
    // earlier in the log.
    let log = HookEventLog::new();
    log.record(HookReport::new(GRANT, HookEvent::Notification));
    log.record(HookReport::new(GRANT, HookEvent::PostToolUse));
    assert_eq!(
        hook_attention(HarnessProvider::Claude, Some(GRANT), false, &log),
        None
    );
}
