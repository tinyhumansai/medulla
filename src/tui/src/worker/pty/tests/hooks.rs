//! A live child's screen and its harness's own lifecycle reports, together.
//!
//! Split out of [`super::attention`] (the 500-line ceiling — see AGENTS.md):
//! these drive a real pty *and* a [`HookEventLog`] together, covering the
//! staleness grace, its open-ended exemption, the output-quiet corroboration,
//! and their interplay with the bell watermark. [`super::attention`] covers
//! everything screen- and bell-only.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use medulla::harness_hooks::{HookEvent, HookEventLog, HookReport};

use super::super::attention::AttentionKind;
use super::{sh, wait_for, PtyManager};

/// The exact race `working_state`'s report-age grace exists for: a
/// lifecycle report still claims the turn is active (its `Stop` has not
/// landed) while the screen has already settled and rung its completion
/// bell. That bell must not be silently discarded the moment it is sampled —
/// it has to survive until the report catches up, or the operator never
/// hears that the turn finished.
#[test]
fn a_completion_bell_during_the_hook_stop_race_is_not_lost() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let hooks = HookEventLog::new();
    manager.set_hook_log(hooks.clone());

    let mut spec = sh("printf 'turn complete\\a\\n'; sleep 30");
    spec.mcp_grant_session = Some("grant-1".to_string());
    let id = manager.open(spec).expect("a session");

    // The harness's own tool report, not yet followed by `Stop`.
    hooks.record(HookReport::new("grant-1", HookEvent::PostToolUse));

    wait_for("the completion output to reach the emulator", || {
        super::screen_text(&manager, &id).contains("turn complete")
    });
    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        manager.attention(&id),
        None,
        "still inside the hook race: no cue yet, but the bell must not be lost either"
    );

    // `Stop` lands.
    hooks.record(HookReport::new("grant-1", HookEvent::Stop));
    now.store(500, Ordering::SeqCst);
    wait_for(
        "the deferred completion bell to surface once the hook clears",
        || {
            manager
                .attention(&id)
                .is_some_and(|cue| cue.kind == AttentionKind::Bell)
        },
    );
    manager.shutdown();
}

/// The gap the coordinator's sanity check was aimed at: a tool that runs
/// longer than `HOOK_STALE_GRACE_MS` (a slow build under `Bash`, say) leaves
/// `PreToolUse` as the last report for the whole run, with nothing recognisable
/// as Claude's own spinner on screen — just the tool's raw output. That report
/// must not be aged out just because the tool is taking a while: it is
/// open-ended by construction (`hook_report_is_open_ended`), so the row must
/// stay `working` for as long as it remains the last report, however long that
/// is.
#[test]
fn a_slow_tool_stays_working_past_the_stale_hook_grace() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let hooks = HookEventLog::new();
    manager.set_hook_log(hooks.clone());

    let mut spec = sh("printf 'running a slow build\\n'; sleep 30");
    spec.mcp_grant_session = Some("grant-1".to_string());
    let id = manager.open(spec).expect("a session");

    // The tool started and has not returned. `PreToolUse` stays the last report
    // for as long as it keeps running — there is no `PostToolUse` to follow it
    // until the tool actually finishes.
    hooks.record(HookReport::new("grant-1", HookEvent::PreToolUse));

    wait_for("the tool's own output to reach the emulator", || {
        super::screen_text(&manager, &id).contains("running a slow build")
    });
    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        manager.row(&id).expect("a row").working,
        "genuinely running a tool"
    );

    // Well past the grace a *boundary* report would be aged out at, with no
    // screen spinner of its own and no new report — the tool is simply still
    // running, not a dropped follow-up.
    now.store(90_000, Ordering::SeqCst); // comfortably past the 30s stale-hook grace
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        manager.row(&id).expect("a row").working,
        "an open-ended report must not go stale just because a tool is slow"
    );
    manager.shutdown();
}

/// The contrasting case: once the tool actually returns, `PostToolUse` is a
/// boundary report again, and the same staleness grace that must not apply to
/// `PreToolUse` does apply here.
#[test]
fn a_stuck_post_tool_report_still_goes_stale() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let hooks = HookEventLog::new();
    manager.set_hook_log(hooks.clone());

    let mut spec = sh("printf 'tool finished\\n'; sleep 30");
    spec.mcp_grant_session = Some("grant-1".to_string());
    let id = manager.open(spec).expect("a session");
    hooks.record(HookReport::new("grant-1", HookEvent::PostToolUse));

    wait_for("the tool's output to reach the emulator", || {
        super::screen_text(&manager, &id).contains("tool finished")
    });
    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        manager.row(&id).expect("a row").working,
        "trusted immediately after the tool returns"
    );

    now.store(90_000, Ordering::SeqCst); // comfortably past the 30s stale-hook grace
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        !manager.row(&id).expect("a row").working,
        "a boundary report this stale, with no follow-up, is presumed dropped"
    );
    manager.shutdown();
}

/// The other half of the coordinator's sanity check: `UserPromptSubmit` is a
/// *boundary* report (subject to the stale-hook grace, unlike `PreToolUse`),
/// on the reasoning that visible activity should reappear within moments of
/// one. That reasoning does not hold for a long, toolless reply: there is no
/// hook event bracketing "now writing the answer", so `UserPromptSubmit` can
/// legitimately remain the last report for as long as the reply takes to
/// generate, and per this module's own docs a reply being written out is one
/// of the paints with no spinner on it. The pty's own live output — not the
/// report's age, not a recognised screen marker — is what must keep this
/// turn `working` past the grace window.
#[test]
fn a_long_toolless_reply_stays_working_while_it_streams() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let hooks = HookEventLog::new();
    manager.set_hook_log(hooks.clone());

    // No tool calls: the model goes straight from the prompt to writing its
    // answer, one line every 50ms, well past this test's real running time.
    let mut spec = sh(
        "i=1; while [ $i -le 60 ]; do printf 'reply line %s\\n' $i; sleep 0.05; i=$((i+1)); done; sleep 30",
    );
    spec.mcp_grant_session = Some("grant-1".to_string());
    let id = manager.open(spec).expect("a session");
    hooks.record(HookReport::new("grant-1", HookEvent::UserPromptSubmit));

    wait_for("the reply to start streaming", || {
        super::screen_text(&manager, &id).contains("reply line 1")
    });
    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        manager.row(&id).expect("a row").working,
        "genuinely streaming a reply"
    );

    // Jump well past the report's own staleness grace. `UserPromptSubmit`,
    // recorded at t=0, is now ancient by that clock alone — but the reply is
    // still visibly streaming in real time, refreshing the pty's own output
    // clock, which must keep this turn `working` regardless of the report's
    // age.
    now.store(90_000, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        manager.row(&id).expect("a row").working,
        "output is still streaming; an ancient report must not end the turn mid-reply"
    );
    manager.shutdown();
}

/// Documented, accepted hole in the `output_quiet_ms` corroboration: a
/// raw-mode TUI echoes keystrokes by repainting its own input line, so an
/// operator typing at an idle composer produces genuine pty output that is
/// indistinguishable here from the harness's own progress. In the exact
/// scenario the staleness grace exists for — a dropped `Stop`, a boundary
/// report stuck as the session's last — this means active typing can hold the
/// row `working` for as long as the operator keeps typing. See the comment on
/// `output_quiet_ms` in `manager::attention::refresh` for why this is judged
/// narrow enough to accept rather than build a second mechanism for. This
/// test exists to make the behaviour explicit and catch it moving, not to
/// approve of it moving silently.
#[test]
fn an_operators_own_keystrokes_can_hold_a_stale_report_working() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let hooks = HookEventLog::new();
    manager.set_hook_log(hooks.clone());

    // `cat` echoes back whatever it reads — standing in for a raw-mode CLI's
    // own input line repainting on every keystroke, not a kernel tty echo.
    let mut spec = sh("cat");
    spec.mcp_grant_session = Some("grant-1".to_string());
    let id = manager.open(spec).expect("a session");
    hooks.record(HookReport::new("grant-1", HookEvent::PostToolUse));

    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        manager.row(&id).expect("a row").working,
        "the fresh report is still trusted"
    );

    // Past the grace, with no further report and no output at all: the
    // dropped-report case this whole mechanism exists for.
    now.store(90_000, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        !manager.row(&id).expect("a row").working,
        "genuinely idle and silent, correctly not working"
    );

    // The operator starts typing. The echoed keystroke is new pty output with
    // no way to tell it apart from real progress.
    manager.write(&id, b"h").expect("type a character");
    wait_for("the echoed keystroke to reach the emulator", || {
        super::screen_text(&manager, &id).contains('h')
    });
    // A new classifier poll, not just the passage of time: the throttle only
    // lets the classifier run again once `now` has moved past the last
    // successful sample.
    now.store(90_200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        manager.row(&id).expect("a row").working,
        "documented hole: an operator's own keystrokes hold a stale report working"
    );
    manager.shutdown();
}
