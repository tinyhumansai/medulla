//! A live child's screen turning into the "this harness wants you" flag.
//!
//! The classifier has its own unit tests against fixed strings; these drive a
//! real pty so the wiring is covered too — the reader thread refreshing, the
//! throttle, the bell counter, and the row the UI actually reads.

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
fn releasing_a_reusable_turn_consumes_an_unclassified_completion_bell() {
    let manager = PtyManager::new();
    let id = manager
        .open(sh("printf 'turn complete\\a\\n'; sleep 30"))
        .expect("a session");

    // Release as soon as the reader has painted the output, before the 200 ms
    // attention poller has classified its bell. This is the production race:
    // finish_turn can release while the completion chime is pending.
    wait_for("the completion output to reach the emulator", || {
        super::screen_text(&manager, &id).contains("turn complete")
    });
    manager.release(&id);

    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(manager.attention(&id), None);
    assert_eq!(manager.waiting_count(), 0);
    manager.shutdown();
}

#[test]
fn releasing_consumes_a_completion_bell_emitted_just_after_settlement() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh("read line; printf 'late completion\\a\\n'; sleep 30"))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    manager.release(&id);
    assert!(
        !manager.acknowledge(&id),
        "the late bell does not exist yet"
    );
    manager
        .claim_idle(&row.label, row.provider)
        .expect("reuse can begin before the old chime arrives");
    manager.write(&id, b"\r").expect("release the child");
    wait_for("the late completion bell to reach the emulator", || {
        super::screen_text(&manager, &id).contains("late completion")
    });

    // The first eligible poll consumes the post-release bell. Later polls must
    // not resurrect it.
    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    now.store(1_000, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(manager.attention(&id), None);
    manager.shutdown();
}

#[test]
fn claiming_the_next_turn_makes_its_bell_meaningful_again() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh("read first; printf 'old completion\\a\\n'; \
             read second; printf 'next turn needs you\\a\\n'; sleep 30"))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    manager.release(&id);
    manager.write(&id, b"\r").expect("emit the old chime");
    wait_for("the old completion bell to reach the emulator", || {
        super::screen_text(&manager, &id).contains("old completion")
    });
    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(manager.attention(&id), None);

    manager
        .claim_idle(&row.label, row.provider)
        .expect("claim the reusable session");
    manager.write(&id, b"\r").expect("emit the new turn bell");
    wait_for("the next turn bell to reach the emulator", || {
        super::screen_text(&manager, &id).contains("next turn needs you")
    });

    now.store(500, Ordering::SeqCst);
    wait_for("the next turn bell to be classified", || {
        manager
            .attention(&id)
            .is_some_and(|cue| cue.kind == AttentionKind::Bell)
    });
    manager.shutdown();
}

#[test]
fn a_consumed_completion_bell_does_not_hide_the_next_turns_first_bell() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh("read first; printf 'old completion\\a\\n'; read second; printf 'next request\\a\\n'; sleep 30"))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    manager.write(&id, b"\r").expect("emit completion bell");
    wait_for("the completion bell to arrive before release", || {
        super::screen_text(&manager, &id).contains("old completion")
    });
    manager.release(&id);
    manager
        .claim_idle(&row.label, row.provider)
        .expect("reuse the session");
    manager.write(&id, b"\r").expect("emit next turn bell");
    wait_for("the next turn request to paint", || {
        super::screen_text(&manager, &id).contains("next request")
    });

    now.store(200, Ordering::SeqCst);
    wait_for("the reused turn's first bell to be classified", || {
        manager
            .attention(&id)
            .is_some_and(|cue| cue.kind == AttentionKind::Bell)
    });
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
