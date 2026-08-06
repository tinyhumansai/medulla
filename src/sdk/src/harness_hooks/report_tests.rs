//! The log's bounded window, its per-session reads, and the "what is this
//! session doing" answer the rail depends on.

use super::*;

fn report(session: &str, event: HookEvent, at_ms: i64) -> HookReport {
    HookReport {
        at_ms,
        ..HookReport::new(session, event)
    }
}

#[test]
fn recent_is_newest_first_and_bounded_by_the_limit() {
    let log = HookEventLog::new();
    log.record(report("a", HookEvent::SessionStart, 1));
    log.record(report("a", HookEvent::Stop, 2));

    let recent = log.recent(10);
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].event, HookEvent::Stop);
    assert_eq!(log.recent(1).len(), 1);
}

#[test]
fn the_window_drops_the_oldest_but_keeps_counting() {
    let log = HookEventLog::new();
    for at in 0..(CAPACITY as i64 + 10) {
        log.record(report("a", HookEvent::PostToolUse, at));
    }
    assert_eq!(log.recent(usize::MAX).len(), CAPACITY);
    assert_eq!(log.recorded(), CAPACITY as u64 + 10);
    // The oldest ten aged out, so the tail of the window starts at 10.
    let oldest = log.recent(usize::MAX).pop().expect("a window");
    assert_eq!(oldest.at_ms, 10);
}

#[test]
fn one_session_never_reads_anothers_events() {
    let log = HookEventLog::new();
    log.record(report("mine", HookEvent::SessionStart, 1));
    log.record(report("theirs", HookEvent::Stop, 2));

    let mine = log.recent_for("mine", 10);
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].session, "mine");
    assert_eq!(log.last_event("mine"), Some(HookEvent::SessionStart));
    assert_eq!(log.last_event("theirs"), Some(HookEvent::Stop));
    assert_eq!(log.last_event("nobody"), None);
}
