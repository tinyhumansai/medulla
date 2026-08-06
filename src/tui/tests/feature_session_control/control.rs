//! Moving control of a session between the operator and the orchestrator.
//!
//! The rule these all circle: a question is asked only when control actually
//! moves. Going into a session you already hold asks nothing, going into one the
//! orchestrator holds asks first, and letting go asks only about a session the
//! orchestrator has a claim on.

use crate::helpers::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use medulla::protocol::HarnessProvider;
use medulla_tui::worker::pty::{PtyManager, SessionControl};

#[test]
fn ctrl_g_hands_a_harness_over_and_back() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);

    // Put the cursor on the harness row; the pane resolves it on that draw.
    let _ = render(&mut app, 140, 44);
    let rows = sessions.rows().len();
    assert_eq!(rows, 1);
    select_harness_row(&mut app);

    let _ = app.on_event(ctrl('g'));
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::Orchestrator,
        "give: {}",
        app.status()
    );
    assert!(app.status().contains("Handed back"), "{}", app.status());
    // …and now dispatch may reuse it, which is the point of handing it over.
    assert!(sessions
        .claim_idle("you:codex", HarnessProvider::Codex)
        .is_some());
    sessions.release(&id);

    let _ = app.on_event(ctrl('g'));
    assert_eq!(sessions.row(&id).unwrap().control, SessionControl::User);
    assert!(
        sessions
            .claim_idle("you:codex", HarnessProvider::Codex)
            .is_none(),
        "taking it back must close dispatch off again"
    );

    sessions.shutdown();
}

#[test]
fn attaching_takes_control_and_releasing_asks_for_it_back() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);
    // Start it under the orchestrator, so attaching is what takes it.
    sessions.set_control(&id, SessionControl::Orchestrator);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);

    let _ = app.on_event(focus_chord());
    assert_eq!(app.attached_session(), Some(id.as_str()));
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::User,
        "focusing in takes the harness: {}",
        app.status()
    );

    // Releasing asks rather than deciding.
    let _ = app.on_event(focus_chord());
    let out = render(&mut app, 140, 44);
    assert!(out.contains("You still have this session"), "{out}");
    assert_eq!(
        app.attached_session(),
        Some(id.as_str()),
        "the keyboard stays put until the question is answered"
    );

    let _ = app.on_event(key(KeyCode::Char('y')));
    assert_eq!(app.attached_session(), None);
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::Orchestrator
    );

    sessions.shutdown();
}

#[test]
fn keeping_a_harness_releases_the_keyboard_but_not_control() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);
    sessions.set_control(&id, SessionControl::Orchestrator);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(focus_chord());

    let _ = app.on_event(key(KeyCode::Char('n')));
    assert_eq!(app.attached_session(), None, "the keyboard comes back");
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::User,
        "but the harness does not"
    );
    assert!(app.status().contains("still hold"), "{}", app.status());

    sessions.shutdown();
}

// Unix-only: stands a real session up on a real pseudo-terminal via `/bin/sh`,
// which Windows has no equivalent of.
#[cfg(unix)]
#[test]
fn a_second_attachment_does_not_inherit_the_first_ones_takeover() {
    // "I took this off the orchestrator" is what a write failure reads to decide
    // whether to give a dead session back, and it used to be a single flag that
    // nothing cleared on release. A `true` left by an earlier attachment
    // survived into the next one — and the next one might be a session the
    // operator started themselves, which the failure would then hand to the
    // orchestrator behind their back. Keyed by session, an attachment that took
    // nothing has nothing to inherit.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let taken = user_session(&sessions);
    let own = user_session(&sessions);
    sessions.set_control(&taken, SessionControl::Orchestrator);

    // First attachment: this one really did take a session off the orchestrator,
    // and the operator keeps it on the way out — so the record of the take
    // outlives the attachment that made it.
    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);
    assert_eq!(app.pane_session_for_test(), Some(taken.as_str()));
    let _ = app.on_event(focus_chord());
    assert_eq!(sessions.row(&taken).unwrap().control, SessionControl::User);
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(key(KeyCode::Char('n')));
    assert_eq!(app.attached_session(), None);
    assert_eq!(sessions.row(&taken).unwrap().control, SessionControl::User);

    // Second attachment, onto the session the operator started themselves:
    // nothing is taken, because it was never the orchestrator's.
    let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
    let _ = render(&mut app, 140, 44);
    assert_eq!(app.pane_session_for_test(), Some(own.as_str()));
    let _ = app.on_event(focus_chord());
    assert_eq!(app.attached_session(), Some(own.as_str()));

    // The child goes away, so the next keystroke fails on a dead pty.
    sessions.shutdown();
    wait_for("the session to stop accepting writes", || {
        sessions.write(&own, b"x").is_err()
    });
    type_str(&mut app, "x");

    assert_eq!(app.attached_session(), None, "a dead pane is detached");
    assert_eq!(
        sessions.row(&own).unwrap().control,
        SessionControl::User,
        "the operator started this one, so it stays theirs: {}",
        app.status()
    );
}

#[test]
fn escape_from_the_question_stays_attached() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);
    sessions.set_control(&id, SessionControl::Orchestrator);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(key(KeyCode::Esc));

    assert_eq!(
        app.attached_session(),
        Some(id.as_str()),
        "Esc is 'I did not mean to leave'"
    );
    assert_eq!(sessions.row(&id).unwrap().control, SessionControl::User);
    let out = render(&mut app, 140, 44);
    assert!(!out.contains("You still have this session"), "{out}");

    sessions.shutdown();
}

#[test]
fn clicking_away_from_an_attached_harness_honors_handback_policy() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);
    sessions.set_control(&id, SessionControl::Orchestrator);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);
    let _ = app.on_event(focus_chord());
    assert_eq!(sessions.row(&id).unwrap().control, SessionControl::User);

    // The Overview tab begins at column zero on the second screen row.
    let _ = app.on_event(click(1, 1));
    let out = render(&mut app, 140, 44);
    assert!(
        out.contains("You still have this session"),
        "pointer navigation must ask before hiding the held pane: {out}"
    );
    assert_eq!(app.attached_session(), Some(id.as_str()));
    assert_eq!(app.tab(), "Agents", "the pending release blocks navigation");

    let _ = app.on_event(key(KeyCode::Char('y')));
    assert_eq!(app.attached_session(), None);
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::Orchestrator
    );

    sessions.shutdown();
}

#[test]
fn enter_on_a_harness_you_already_hold_just_types_into_it() {
    // The reported confusion. Enter on a harness row raised the handover
    // question, so the answer to "let me type in this" was an offer to give the
    // harness away — over the pane being aimed at, and with Escape as its only
    // useful answer. With operator-started sessions unmanaged, that was every
    // session on the rail.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);

    let _ = app.on_event(key(KeyCode::Enter));

    assert_eq!(
        app.attached_session(),
        Some(id.as_str()),
        "Enter on a harness you hold means type into it: {}",
        app.status()
    );
    let out = render(&mut app, 140, 44);
    assert!(
        !out.contains("still have this session") && !out.contains("Take control"),
        "there is nothing to negotiate about a harness that was always yours: {out}"
    );
    assert_eq!(sessions.row(&id).unwrap().control, SessionControl::User);

    sessions.shutdown();
}

#[test]
fn enter_on_an_orchestrator_harness_still_asks_before_taking_it() {
    // The other half of the same rule, and the case the question exists for:
    // Enter is a navigation key, so walking the rail onto a harness dispatch is
    // using must not lock the orchestrator out of that workspace silently.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);
    sessions.set_control(&id, SessionControl::Orchestrator);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);

    let _ = app.on_event(key(KeyCode::Enter));

    let out = render(&mut app, 140, 44);
    assert!(out.contains("Take control of this session"), "{out}");
    assert_eq!(app.attached_session(), None, "asking is not taking");
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::Orchestrator,
        "control must not move until the question is answered"
    );

    let _ = app.on_event(key(KeyCode::Enter));
    assert_eq!(app.attached_session(), Some(id.as_str()));
    assert_eq!(sessions.row(&id).unwrap().control, SessionControl::User);

    sessions.shutdown();
}

#[test]
fn releasing_a_harness_you_started_yourself_asks_nothing() {
    // `Ask` is the default policy, and the question it asks is "the orchestrator
    // is locked out of this — shall I give it back?". For a session the operator
    // started, that premise is false: dispatch never had it. Asking anyway put a
    // modal in front of every `Ctrl-]`, which is how a confirmation stops being
    // read.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);

    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);
    let _ = app.on_event(focus_chord());
    assert_eq!(app.attached_session(), Some(id.as_str()));

    let _ = app.on_event(focus_chord());

    assert_eq!(
        app.attached_session(),
        None,
        "the keyboard comes straight back"
    );
    let out = render(&mut app, 140, 44);
    assert!(!out.contains("You still have this session"), "{out}");
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::User,
        "and the harness is still the operator's"
    );

    sessions.shutdown();
}

#[test]
fn a_kept_harness_is_asked_about_again_but_a_neighbour_is_not() {
    // Two harnesses, one taken from the orchestrator and kept. Whether the
    // release question is owed was a single flag, so holding one taken harness
    // made *every* harness ask — including one the operator started, which
    // dispatch had never touched.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let taken = user_session(&sessions);
    let own = user_session(&sessions);
    sessions.set_control(&taken, SessionControl::Orchestrator);

    // Take the first by focusing into it, then keep it on the way out.
    let _ = render(&mut app, 140, 44);
    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);
    assert_eq!(app.pane_session_for_test(), Some(taken.as_str()));
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(key(KeyCode::Char('n')));
    assert_eq!(sessions.row(&taken).unwrap().control, SessionControl::User);

    // Step onto the harness the operator started and go in and out of it.
    let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
    let _ = render(&mut app, 140, 44);
    assert_eq!(app.pane_session_for_test(), Some(own.as_str()));
    let _ = app.on_event(focus_chord());
    assert_eq!(app.attached_session(), Some(own.as_str()));
    let _ = app.on_event(focus_chord());

    let out = render(&mut app, 140, 44);
    assert!(
        !out.contains("You still have this session"),
        "the neighbouring harness was never the orchestrator's: {out}"
    );
    assert_eq!(app.attached_session(), None);

    // The taken one is still owed the question, and still worded as a take.
    let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)));
    let _ = render(&mut app, 140, 44);
    assert_eq!(app.pane_session_for_test(), Some(taken.as_str()));
    let _ = app.on_event(focus_chord());
    let _ = app.on_event(focus_chord());
    let out = render(&mut app, 140, 44);
    assert!(out.contains("You still have this session"), "{out}");

    sessions.shutdown();
}

#[test]
fn releasing_a_task_spawned_session_asks_even_when_it_was_never_taken() {
    // The regression Codex caught on #199. Deciding "is a question owed?" from
    // the taken-map alone assumed every user-held session missing from it was
    // one the operator started. It is not: the executor hands a failed reusable
    // turn straight to the operator (`worker::executor::run`) without going
    // through `take_session`, so attaching to see what went wrong and pressing
    // the focus chord released in silence — leaving an orchestrator-originated
    // session user-held, with dispatch locked out of it until the operator
    // happened to discover `/handoff`.
    //
    // Origin is the durable fact and is what the rule keys on now.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = orchestrator_session(&sessions);
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::User,
        "the executor hands it over already user-held"
    );

    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);
    let _ = app.on_event(focus_chord());
    assert_eq!(app.attached_session(), Some(id.as_str()));

    let _ = app.on_event(focus_chord());

    let out = render(&mut app, 140, 44);
    assert!(
        out.contains("You still have this session"),
        "a session dispatch created is one the orchestrator has a claim on: {out}"
    );
    assert_eq!(
        app.attached_session(),
        Some(id.as_str()),
        "the keyboard stays put until the question is answered"
    );

    let _ = app.on_event(key(KeyCode::Char('y')));
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::Orchestrator,
        "answering yes gives dispatch its session back"
    );

    sessions.shutdown();
}

#[test]
fn releasing_a_session_you_gave_away_and_got_back_asks_again() {
    // The other half of the claim rule. `SessionOrigin` alone under-counts:
    // a session the operator started stays origin `User` forever, but once
    // handed over it is genuinely dispatchable — `SessionHandle::serves_label`
    // adopts a handed-back operator session for a task. The executor can then
    // hand a failed turn straight back user-held, with origin `User` and no take
    // recorded, and releasing that in silence leaves dispatch locked out.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);

    select_harness_row(&mut app);
    let _ = render(&mut app, 140, 44);

    // Give it away, which is what makes it dispatchable from here on.
    let _ = app.on_event(ctrl('g'));
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::Orchestrator
    );

    // Now the executor's failed-reusable-turn path: control back to the operator
    // without passing through `take_session`.
    sessions.set_control(&id, SessionControl::User);

    let _ = render(&mut app, 140, 44);
    let _ = app.on_event(focus_chord());
    assert_eq!(app.attached_session(), Some(id.as_str()));
    let _ = app.on_event(focus_chord());

    let out = render(&mut app, 140, 44);
    assert!(
        out.contains("You still have this session"),
        "a session the orchestrator has had a claim on is owed the question: {out}"
    );

    sessions.shutdown();
}

#[test]
fn ctrl_g_refuses_a_remote_row_rather_than_handing_back_something_else() {
    // With the cursor on a session hosted elsewhere, `pane_session` is empty.
    // Resolving the chord through the single-held-session fallback would then
    // hand back an unrelated *local* session the operator was not looking at —
    // silently, which is worse than the refusal.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = user_session(&sessions);
    let _ = render(&mut app, 140, 44);
    app.set_pane_remote_session_for_test(Some("dev-2:codex".to_string()));

    let _ = app.on_event(ctrl('g'));

    assert!(
        app.status().contains("another host"),
        "the refusal must name the machine: {}",
        app.status()
    );
    assert_eq!(
        sessions.row(&id).unwrap().control,
        SessionControl::User,
        "the unrelated local session must not have been handed back"
    );

    sessions.shutdown();
}
