//! A live child's screen turning into the "this harness wants you" flag.
//!
//! The classifier has its own unit tests against fixed strings; these drive a
//! real pty so the wiring is covered too — the reader thread refreshing, the
//! throttle, and cue recognition itself: permission prompts, dialogs, and the
//! screen/hook classification that decides whether a bell is a progress chime
//! or a question. [`super::settlement`] covers the bell watermark and
//! completion-chime debt across `settle_turn`/`claim_idle`/`release` — a
//! session's turn-to-turn handoff, once a cue already exists to hand off.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use super::super::attention::AttentionKind;
use super::{sh, wait_for, PtyManager};

/// Codex's approval prompt, printed by `/bin/sh` and then held on screen.
///
/// Codex's wording because [`sh`] launches its specs as that provider, and the
/// marker tables are per-harness — a claude prompt on a codex session is not
/// something the classifier should believe.
///
/// `sleep` rather than exit: an exited session has nothing to be waiting for,
/// and the flag is about a *running* harness.
const PROMPT_SCRIPT: &str = "printf 'Allow Codex to run `ls`?\\n  1. Yes, proceed\\n  3. No, and tell Codex what to do differently\\n'; sleep 30";

#[test]
fn a_harness_sitting_on_a_permission_prompt_asks_for_the_operator() {
    let manager = PtyManager::new();
    let id = manager.open(sh(PROMPT_SCRIPT)).expect("a session");

    wait_for("the prompt to be classified", || {
        manager.attention(&id).is_some()
    });

    let cue = manager.attention(&id).expect("a cue");
    assert_eq!(cue.kind, AttentionKind::Approval);
    assert_eq!(manager.waiting_count(), 1);
    // The row the UI reads carries it, not only the manager.
    assert!(manager.row(&id).expect("a row").attention.is_some());
    manager.shutdown();
}

#[test]
fn acknowledging_a_live_prompt_takes_the_cue_off_the_row() {
    let manager = PtyManager::new();
    let id = manager.open(sh(PROMPT_SCRIPT)).expect("a session");
    wait_for("the prompt to be classified", || {
        manager.attention(&id).is_some()
    });

    // The return value is the assertion, deliberately. The prompt is *still on
    // screen*, so the next 200ms sample legitimately puts the cue back — that is
    // the documented behaviour of a named cue, and asserting `attention == None`
    // after this line would be racing the poller rather than testing anything.
    // What is deterministic, and what the attach path depends on, is that this
    // call found a cue and removed it.
    assert!(manager.acknowledge(&id), "a live cue to clear");
    manager.shutdown();
}

#[test]
fn acknowledging_consumes_an_unclassified_bell() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh("printf 'ready for you\\a\\n'; sleep 30"))
        .expect("a session");

    wait_for("the bell output to reach the emulator", || {
        super::screen_text(&manager, &id).contains("ready for you")
    });
    assert!(!manager.acknowledge(&id), "the poller has not stored a cue");

    // Let classification resume. It must see the bell as already consumed by
    // the acknowledgment rather than installing it after the operator leaves.
    now.store(1_000, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(manager.attention(&id), None);
    manager.shutdown();
}

#[test]
fn a_prompt_that_leaves_the_screen_stops_asking() {
    let manager = PtyManager::new();
    // Prompt, then clear the screen and carry on working: the flag must follow
    // the screen rather than latch, or every harness that ever asked anything
    // would blink for the rest of its life.
    let script = "printf 'Allow Codex to run `ls`?\\n  3. No, and tell Codex what to do differently\\n'; \
                  sleep 1; printf '\\033[2J\\033[H'; printf 'working… (esc to interrupt)\\n'; sleep 30";
    let id = manager.open(sh(script)).expect("a session");

    wait_for("the prompt to be classified", || {
        manager.attention(&id).is_some()
    });
    wait_for("the prompt to clear", || manager.attention(&id).is_none());
    manager.shutdown();
}

#[test]
fn a_bell_from_an_idle_harness_asks_for_the_operator() {
    let manager = PtyManager::new();
    // No recognisable words at all — just the bell every harness rings when it
    // wants a human. This is the cue that works on a CLI whose prompts we have
    // never seen.
    let id = manager
        .open(sh("printf 'all done\\a\\n'; sleep 30"))
        .expect("a session");

    wait_for("the bell to be noticed", || {
        manager.attention(&id).is_some()
    });
    assert_eq!(
        manager.attention(&id).expect("a cue").kind,
        AttentionKind::Bell
    );
    manager.shutdown();
}

#[test]
fn attention_sampling_ignores_the_operators_historical_viewport() {
    let manager = PtyManager::new();
    let script = "printf 'Allow Codex to run `old`?\\n› 1. Yes, proceed\\n'; \
                  i=1; while [ $i -le 35 ]; do printf 'line %s\\n' $i; i=$((i+1)); done; \
                  printf 'live ordinary screen\\n'; sleep 30";
    let id = manager.open(sh(script)).expect("a session");

    wait_for("the live screen to settle", || {
        manager
            .tail_lines(&id, 50)
            .iter()
            .any(|line| line == "live ordinary screen")
    });
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(manager.attention(&id), None);
    let offset = manager.scroll_history(&id, 12, true).expect("a session");
    assert!(offset > 0);
    assert!(
        super::screen_text(&manager, &id).contains("Allow Codex"),
        "the historical viewport should contain the old prompt"
    );

    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(manager.attention(&id), None);
    assert_eq!(manager.scroll_history(&id, 0, true), Some(offset));
    manager.shutdown();
}

#[test]
fn a_bell_rung_mid_turn_is_a_progress_chime_not_a_question() {
    let manager = PtyManager::new();
    // The interrupt footer says the harness is working; a bell alongside it is
    // a tool finishing, not a request. Nothing to wait *for* here, so the check
    // is that a settled screen produces no cue.
    let id = manager
        .open(sh("printf 'thinking… (esc to interrupt)\\a\\n'; sleep 30"))
        .expect("a session");

    wait_for("the screen to paint", || {
        super::screen_text(&manager, &id).contains("thinking")
    });
    // Long enough that the throttled refresh has certainly run on that screen.
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(manager.attention(&id), None);
    manager.shutdown();
}

/// A bell sampled while the *screen itself* plainly shows live work must be
/// consumed immediately, not deferred: deferral exists only for the narrow
/// case where `working` is true solely because a stale-tolerant hook report
/// says so while the screen already looks idle. Deferring every bell whenever
/// `working` happened to be true for any reason would instead let an ordinary
/// progress chime survive the whole rest of the turn and resurface as a false
/// completion cue the moment it silently ends.
#[test]
fn a_progress_bell_seen_during_a_live_spinner_does_not_resurface_at_completion() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh(
            "printf 'thinking… (esc to interrupt)\\a\\n'; read line; printf '\\033[2J\\033[Hdone\\n'; sleep 30",
        ))
        .expect("a session");

    wait_for("the progress bell to paint", || {
        super::screen_text(&manager, &id).contains("thinking")
    });
    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        manager.attention(&id),
        None,
        "a spinner-visible progress bell produces no cue, same as always"
    );

    // The turn ends silently: no second, distinct completion bell.
    manager.write(&id, b"\r").expect("end the turn");
    wait_for("the settled screen to paint", || {
        super::screen_text(&manager, &id).contains("done")
    });
    now.store(500, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        manager.attention(&id),
        None,
        "the earlier progress chime, safely consumed while genuinely working, must not resurface as a false completion cue"
    );
    manager.shutdown();
}

#[test]
fn an_ordinary_screen_asks_for_nothing() {
    let manager = PtyManager::new();
    let id = manager
        .open(sh("printf 'hello from a harness\\n'; sleep 30"))
        .expect("a session");

    wait_for("the screen to paint", || {
        super::screen_text(&manager, &id).contains("hello")
    });
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(manager.attention(&id), None);
    assert_eq!(manager.waiting_count(), 0);
    manager.shutdown();
}
