//! A session's turn-to-turn handoff: `settle_turn`, `claim_idle`, `release`,
//! and the bell watermark and completion-chime debt that carry across them.
//!
//! Split out of [`super::attention`] (the 500-line ceiling — see AGENTS.md)
//! along the seam between *recognising* a cue and *handing a session off*
//! between turns: these tests all drive `settle_turn`/`claim_idle`/`release`
//! and check what a promised or observed bell does across that boundary —
//! [`super::attention`] covers cue recognition (permission prompts, dialogs,
//! the screen/hook classification a bell is checked against) on its own,
//! before any turn handoff enters the picture.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use super::super::attention::AttentionKind;
use super::{sh, wait_for, PtyManager, SessionControl};

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
    manager.settle_turn(&id);

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
        .open(sh(
            "sleep 0.15; printf 'late completion\\a\\n'; read line; sleep 30",
        ))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    manager.settle_turn(&id);
    assert!(
        !manager.acknowledge(&id),
        "early acknowledgment finds no cue but preserves the promised chime"
    );
    manager
        .claim_idle(&row.label, row.provider)
        .expect("reuse waits for the old chime window");
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

    manager.settle_turn(&id);
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
    manager.settle_turn(&id);
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
fn a_classified_completion_bell_does_not_hide_the_next_turns_first_bell() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh("read first; printf 'old completion\\a\\n'; read second; printf 'next request\\a\\n'; sleep 30"))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    manager.write(&id, b"\r").expect("emit completion bell");
    wait_for("the completion bell to paint", || {
        super::screen_text(&manager, &id).contains("old completion")
    });
    now.store(200, Ordering::SeqCst);
    wait_for("the completion bell to be classified", || {
        manager
            .attention(&id)
            .is_some_and(|cue| cue.kind == AttentionKind::Bell)
    });

    manager.settle_turn(&id);
    assert_eq!(manager.attention(&id), None);
    manager
        .claim_idle(&row.label, row.provider)
        .expect("reuse the settled session");
    manager.write(&id, b"\r").expect("emit next turn bell");
    wait_for("the next request to paint", || {
        super::screen_text(&manager, &id).contains("next request")
    });

    now.store(500, Ordering::SeqCst);
    wait_for("the reused turn's bell to be classified", || {
        manager
            .attention(&id)
            .is_some_and(|cue| cue.kind == AttentionKind::Bell)
    });
    manager.shutdown();
}

#[test]
fn suppression_consumes_only_one_bell_from_a_batch() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh(
            "read line; printf 'late completion\\a next request\\a\\n'; sleep 30",
        ))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    manager.settle_turn(&id);
    manager
        .claim_idle(&row.label, row.provider)
        .expect("reuse before either bell arrives");
    manager.write(&id, b"\r").expect("emit both bells");
    wait_for("both bell outputs to paint", || {
        super::screen_text(&manager, &id).contains("next request")
    });

    now.store(200, Ordering::SeqCst);
    wait_for("the second bell in the batch to remain eligible", || {
        manager
            .attention(&id)
            .is_some_and(|cue| cue.kind == AttentionKind::Bell)
    });
    manager.shutdown();
}

#[test]
fn successive_settlements_track_each_pending_completion_bell() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh(
            "sleep 0.15; printf 'two completions\\a\\a\\n'; read line; sleep 30",
        ))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    manager.settle_turn(&id);
    manager.settle_turn(&id);
    manager
        .claim_idle(&row.label, row.provider)
        .expect("reuse waits for both promised chimes");
    wait_for("both completion bells to paint", || {
        super::screen_text(&manager, &id).contains("two completions")
    });

    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(manager.attention(&id), None);
    manager.shutdown();
}

#[test]
fn freeing_without_a_submitted_turn_does_not_suppress_the_next_bell() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh(
            "read line; printf 'permission fallback\\a\\n'; sleep 30",
        ))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    // This is the injection-failure/claim-rollback path: no turn reached the
    // harness, so there can be no completion bell to suppress.
    manager.release(&id);
    manager
        .claim_idle(&row.label, row.provider)
        .expect("reuse the freed session");
    manager.write(&id, b"\r").expect("emit the first real bell");
    wait_for("the bell output to paint", || {
        super::screen_text(&manager, &id).contains("permission fallback")
    });

    now.store(200, Ordering::SeqCst);
    wait_for("the first real bell to remain eligible", || {
        manager
            .attention(&id)
            .is_some_and(|cue| cue.kind == AttentionKind::Bell)
    });
    manager.shutdown();
}

#[test]
fn a_working_vetoed_completion_does_not_hide_the_next_request() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh("printf 'working (esc to interrupt)\\a\\n'; read line; printf '\\033[2J\\033[Hnext request\\a\\n'; sleep 30"))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    wait_for("the working bell to paint", || {
        super::screen_text(&manager, &id).contains("working")
    });
    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(manager.attention(&id), None);

    manager.settle_turn(&id);
    manager
        .claim_idle(&row.label, row.provider)
        .expect("reuse after the observed completion");
    manager.write(&id, b"\r").expect("emit the next request");
    wait_for("the next request to paint", || {
        super::screen_text(&manager, &id).contains("next request")
    });
    now.store(500, Ordering::SeqCst);
    wait_for("the request bell to remain eligible", || {
        manager
            .attention(&id)
            .is_some_and(|cue| cue.kind == AttentionKind::Bell)
    });
    manager.shutdown();
}

#[test]
fn a_silent_completion_does_not_hide_the_next_turns_bell() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh("read line; printf 'next request\\a\\n'; sleep 30"))
        .expect("a session");
    let row = manager.row(&id).expect("a row");

    manager.settle_turn(&id);
    manager
        .claim_idle(&row.label, row.provider)
        .expect("reuse after a silent turn");
    manager.write(&id, b"\r").expect("submit the next turn");
    wait_for("the next request to paint", || {
        super::screen_text(&manager, &id).contains("next request")
    });

    now.store(200, Ordering::SeqCst);
    wait_for("the next turn's bell to remain eligible", || {
        manager
            .attention(&id)
            .is_some_and(|cue| cue.kind == AttentionKind::Bell)
    });
    manager.shutdown();
}

#[test]
fn expired_completion_debt_does_not_hide_an_operators_next_bell() {
    let manager = PtyManager::new();
    let id = manager
        .open(sh("read line; printf 'operator request\\a\\n'; sleep 30"))
        .expect("a session");

    manager.settle_turn(&id);
    manager.set_control(&id, SessionControl::User);
    std::thread::sleep(std::time::Duration::from_millis(400));
    manager
        .write(&id, b"\r")
        .expect("the operator submits a turn");
    wait_for("the operator request to paint", || {
        super::screen_text(&manager, &id).contains("operator request")
    });
    wait_for("the expired debt to leave the bell eligible", || {
        manager
            .attention(&id)
            .is_some_and(|cue| cue.kind == AttentionKind::Bell)
    });
    manager.shutdown();
}

#[test]
fn an_earlier_progress_bell_does_not_stand_in_for_a_delayed_completion() {
    let now = Arc::new(AtomicI64::new(0));
    let clock = Arc::clone(&now);
    let manager = PtyManager::with_now(Arc::new(move || clock.load(Ordering::SeqCst)));
    let id = manager
        .open(sh("printf 'working (esc to interrupt)\\a\\n'; read line; printf '\\033[2J\\033[Hdone\\a\\n'; sleep 30"))
        .expect("a session");

    wait_for("the progress bell to paint", || {
        super::screen_text(&manager, &id).contains("working")
    });
    now.store(200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(manager.attention(&id), None);

    // Settlement happens well after the progress chime. Its actual completion
    // bell has not been emitted yet and must still be promised for suppression.
    now.store(1_000, Ordering::SeqCst);
    manager.settle_turn(&id);
    manager.write(&id, b"\r").expect("emit the completion bell");
    wait_for("the delayed completion bell to paint", || {
        super::screen_text(&manager, &id).contains("done")
    });

    now.store(1_200, Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(manager.attention(&id), None);
    manager.shutdown();
}
