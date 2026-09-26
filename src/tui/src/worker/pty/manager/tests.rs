//! Unit tests for PTY manager bookkeeping rules.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use portable_pty::{Child, ChildKiller, ExitStatus};

use super::attention::{
    bell_accounting, bells_are_deferred, working_state, HOOK_STALE_GRACE_MS, OPEN_ENDED_ABANDON_MS,
};
use super::open::LaunchGuard;
use super::session::consumed_bell_count;

#[test]
fn a_stale_bell_sample_cannot_move_the_watermark_backwards() {
    // Release sampled one bell, then an in-flight classifier stored the second
    // before release reacquired the sessions lock.
    assert_eq!(consumed_bell_count(2, 1), 2);
}

// `working_state` is a pure function of the booleans/ages `refresh` derives
// from one hook-log snapshot (see `attention::hook::tests` for the log-level
// scoping and event-classification rules that feed `hook_active`, and
// `attention::detect::tests` for `screen_working`), so these tests drive it
// directly with those values rather than re-building a `HookEventLog` per
// case.

#[test]
fn a_live_spinner_and_the_reports_between_them_cover_a_turn() {
    // A composer on screen no longer ends a turn on its own: Claude keeps its
    // prompt drawn for the whole turn, and `PostToolUse` is exactly the middle
    // of one — a tool result painted over the spinner, with the composer below
    // it as always.
    assert!(
        working_state(false, Some(true), None, false, None),
        "a mid-turn report holds the working state through a spinner-less paint"
    );
    assert!(
        !working_state(false, Some(false), None, false, None),
        "the turn's own Stop report ends it"
    );
    assert!(
        working_state(true, Some(false), None, false, None),
        "a fresh spinner starts work before UserPromptSubmit is filed"
    );
}

/// `run_hook_cmd` swallows a report's delivery failure or timeout
/// (`commands/hook.rs`), so a session whose `Stop` report never arrived must
/// not hold `working` forever once the screen has gone idle — that would
/// suppress its completion bell for the rest of the session. Staleness is
/// judged from the report's own age (`report_age_ms`, as the caller would
/// compute from `HookReport::at_ms`), not from how long the screen has looked
/// idle.
#[test]
fn a_dropped_stop_report_does_not_latch_working_forever() {
    assert!(
        working_state(
            false,
            Some(true),
            Some(HOOK_STALE_GRACE_MS - 1),
            false,
            Some(HOOK_STALE_GRACE_MS),
        ),
        "a report just inside the grace window is trusted over the idle screen"
    );
    assert!(
        !working_state(
            false,
            Some(true),
            Some(HOOK_STALE_GRACE_MS),
            false,
            Some(HOOK_STALE_GRACE_MS),
        ),
        "a report this old, with the pty just as quiet, outweighs staying working"
    );
}

/// The report's staleness is only suggestive on its own: there is no hook
/// event bracketing "now writing the reply", so a stale `PostToolUse` or
/// `UserPromptSubmit` can be exactly what a long, spinner-less answer looks
/// like from the hook's side. Fresh output is what tells that apart from a
/// genuinely dropped report.
#[test]
fn fresh_output_corroborates_a_stale_report_that_is_still_a_long_reply() {
    assert!(
        working_state(
            false,
            Some(true),
            Some(HOOK_STALE_GRACE_MS * 10),
            false,
            Some(0)
        ),
        "new output keeps arriving, so the ancient report is still a live reply, not a dropped one"
    );
}

/// A fresh report right after a long-idle screen must be trusted immediately —
/// staleness tracks the report's own age, not a clock started the moment the
/// screen went quiet, so resuming an idle session (a new `UserPromptSubmit`) or
/// the next tool report past a permission menu is not penalised for how long
/// the prompt sat idle before it arrived.
#[test]
fn a_fresh_report_after_a_long_idle_screen_is_trusted_at_once() {
    assert!(working_state(false, Some(true), Some(0), false, None));
}

/// An open-ended report (`PreToolUse`/`SubagentStart`/`PreCompact`) is not
/// aged by `HOOK_STALE_GRACE_MS`, but it is not trusted forever either: if its
/// own closing event never arrives, total pty silence for
/// `OPEN_ENDED_ABANDON_MS` is presumed abandonment rather than a tool that
/// merely produces no output.
#[test]
fn an_abandoned_open_ended_report_eventually_expires() {
    assert!(
        working_state(
            false,
            Some(true),
            Some(HOOK_STALE_GRACE_MS * 100),
            true,
            Some(OPEN_ENDED_ABANDON_MS - 1),
        ),
        "still within the backstop, an open-ended report is trusted regardless of its own age"
    );
    assert!(
        !working_state(
            false,
            Some(true),
            Some(HOOK_STALE_GRACE_MS * 100),
            true,
            Some(OPEN_ENDED_ABANDON_MS),
        ),
        "total silence this long means abandoned even for an open-ended report"
    );
}

/// A *known* fresh age must outrank the abandonment backstop: a report
/// recorded moments ago is direct evidence of life on its own, and silence
/// that ended before the report was even filed says nothing about it. A
/// session silent for the whole `OPEN_ENDED_ABANDON_MS` window that then gets
/// a brand-new boundary report must read `working` at once, not wait for the
/// next pty byte to catch up.
#[test]
fn a_fresh_report_outranks_the_abandon_backstop() {
    assert!(
        working_state(
            false,
            Some(true),
            Some(0),
            false,
            Some(OPEN_ENDED_ABANDON_MS)
        ),
        "a report this fresh is direct evidence of life, whatever the prior silence was"
    );
}

/// The open-ended side of the same property: a fresh `PreToolUse` arriving
/// after `OPEN_ENDED_ABANDON_MS` of silence must also read `working` at once.
/// Folding "open-ended" into `report_age_ms` as `None` made this impossible to
/// say — a `None` age could not also be a *fresh* age, so a brand-new
/// open-ended report had no way to take the fresh-age path and lost to the
/// backstop until the tool's first byte arrived. Keeping `report_open_ended`
/// as its own fact is what lets a report be both at once.
#[test]
fn a_fresh_open_ended_report_outranks_the_abandon_backstop() {
    assert!(
        working_state(
            false,
            Some(true),
            Some(0),
            true,
            Some(OPEN_ENDED_ABANDON_MS)
        ),
        "a fresh PreToolUse is direct evidence of life even after a long silence"
    );
}

/// With no lifecycle report there is nothing to hold the state between paints,
/// so only a spinner says a harness is busy.
#[test]
fn a_session_without_reports_falls_back_to_the_screen() {
    assert!(!working_state(false, None, None, false, None));
    assert!(working_state(true, None, None, false, None));
}

/// The debt consumption and the bell watermark must be made as one decision:
/// a deferred poll must leave *both* the completion debt and the watermark
/// untouched, or the debt gets silently spent on a bell the watermark then
/// also fails to remember — reproducing, on the very next non-deferring
/// poll, exactly the spurious completion cue this pairing exists to prevent.
#[test]
fn a_deferred_poll_leaves_the_completion_debt_and_watermark_paired() {
    // Poll 1: deferring. A promised completion bell has just arrived (`bells`
    // is one past `seen_bells`), but the poll defers because the hook, not
    // the screen, is what says `working`.
    let (consumed, seen_bells) = bell_accounting(true, 1, 1, 0, 1);
    assert_eq!(
        (consumed, seen_bells),
        (0, 0),
        "deferring must leave both the debt and the watermark exactly as they were"
    );

    // Poll 2: the same bell, `working` now cleared. With the debt and
    // watermark both still where poll 1 left them, this must be the decision
    // poll 1 would have made had it not deferred at all — the debt still
    // standing absorbs the bell, leaving nothing eligible.
    let (consumed, seen_bells) = bell_accounting(false, 1, 1, seen_bells, 1);
    assert_eq!(
        (consumed, seen_bells),
        (1, 1),
        "the debt still standing absorbs the deferred bell; nothing is left eligible"
    );
}

/// A bell rung while an open-ended report is still open is a progress chime,
/// not a completion one, and must be accounted at once.
///
/// The tool's own output is what is on screen during a `PreToolUse`, so
/// `screen_working` is false by construction — the spinner is Claude's chrome,
/// not the tool's. Keying deferral on that alone held every bell a running
/// tool emitted (a build that prints one, a shell beeping at an error) and let
/// it resurface as a false "wants you" once the turn ended, for a chime nobody
/// meant as a request.
#[test]
fn a_bell_under_an_open_ended_report_is_not_deferred() {
    assert!(
        !bells_are_deferred(true, false, true),
        "an open-ended report is the harness saying the operation is still \
         open — demonstrably working, so the bell is a progress chime"
    );
    assert!(
        !bells_are_deferred(true, true, false),
        "a spinner on screen is the ordinary working case and never defers"
    );
    assert!(
        bells_are_deferred(true, false, false),
        "a boundary report trusted through a corroboration gap is the one \
         state whose bell may be the completion chime of a finished turn"
    );
    assert!(
        !bells_are_deferred(false, false, false),
        "a session that is not working decides its bells immediately"
    );
}

/// A stand-in child that records whether it was asked to die, since the real
/// `spawn_command`/`try_clone_reader`/`take_writer` sequence `open` runs is
/// not something a test can reliably make fail partway through.
#[derive(Debug)]
struct RecordingChild {
    killed: Arc<AtomicBool>,
    reaped: Arc<AtomicBool>,
}

impl ChildKiller for RecordingChild {
    fn kill(&mut self) -> std::io::Result<()> {
        self.killed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(RecordingChild {
            killed: self.killed.clone(),
            reaped: self.reaped.clone(),
        })
    }
}

impl Child for RecordingChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Ok(None)
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.reaped.store(true, Ordering::SeqCst);
        Ok(ExitStatus::with_exit_code(0))
    }

    fn process_id(&self) -> Option<u32> {
        None
    }

    // Only required on Windows, where `portable_pty::Child` has no default —
    // this stand-in names no real process, so there is no handle to report.
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

/// The property behind the "Revoke grants when PTY setup fails" fix: a
/// `LaunchGuard` dropped while still armed — meaning `open` returned early
/// through one of the fallible calls between `spawn_command` and the
/// `SessionHandle` that would otherwise track this child — must kill the
/// child it already spawned. Left running, that child would keep holding a
/// grant nothing has revoked and nothing else can reap.
#[test]
fn dropping_an_armed_launch_guard_kills_its_child() {
    let killed = Arc::new(AtomicBool::new(false));
    let reaped = Arc::new(AtomicBool::new(false));
    let child: Box<dyn Child + Send + Sync> = Box::new(RecordingChild {
        killed: killed.clone(),
        reaped: reaped.clone(),
    });
    let mut guard = LaunchGuard::new(Some("test-session".to_string()));
    guard.set_child(child);

    drop(guard);

    assert!(
        killed.load(Ordering::SeqCst),
        "an abandoned launch must kill the child it already spawned"
    );
    // Killing is only half of it. This guard is the child's sole owner on the
    // early-return path — the `SessionHandle::reap` that normally waits was
    // never built — so a kill without a wait leaves a zombie holding a
    // process slot until Medulla exits, which is worst exactly when pty setup
    // is failing repeatedly.
    assert!(
        reaped.load(Ordering::SeqCst),
        "a killed child must also be waited on, or it lingers as a zombie"
    );
}

/// The success path must not kill the very child it just spawned: `disarm`
/// hands it to the `SessionHandle` that becomes its permanent owner.
#[test]
fn disarming_a_launch_guard_hands_back_the_child_without_killing_it() {
    let killed = Arc::new(AtomicBool::new(false));
    let reaped = Arc::new(AtomicBool::new(false));
    let child: Box<dyn Child + Send + Sync> = Box::new(RecordingChild {
        killed: killed.clone(),
        reaped: reaped.clone(),
    });
    let mut guard = LaunchGuard::new(None);
    guard.set_child(child);

    let handed_back = guard.disarm();

    assert!(
        handed_back.is_some(),
        "the child must be handed to its permanent owner"
    );
    assert!(
        !killed.load(Ordering::SeqCst),
        "a successful launch must not kill its own child"
    );
    assert!(
        !reaped.load(Ordering::SeqCst),
        "nor wait on it — the handle taking ownership is what reaps it later"
    );
}

/// A guard armed with no child at all (a launch that failed before
/// `spawn_command` even ran) must not panic on drop.
#[test]
fn dropping_a_childless_launch_guard_is_harmless() {
    drop(LaunchGuard::new(Some("test-session-no-child".to_string())));
}
