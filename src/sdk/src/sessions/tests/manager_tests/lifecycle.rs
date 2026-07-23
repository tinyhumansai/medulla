//! Session lifecycle: open, reopen, attachability, reset, close, forget, observe.

use super::*;

#[test]
fn opening_a_session_starts_no_process() {
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));

    let record = manager.record(&id).expect("the session exists");
    assert_eq!(
        record.phase,
        SessionPhase::Idle,
        "an opened-but-unused session must cost nothing"
    );
    assert_eq!(record.class, SessionClass::Unbound);
    assert!(record.is_attachable());
}

#[test]
fn reopening_a_conversation_returns_the_same_session() {
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let first = manager.open(OpenSession::operator("alice"));
    let second = manager.open(OpenSession::operator("alice"));
    assert_eq!(
        first, second,
        "a rival session would split the conversation"
    );
    assert_eq!(manager.records().len(), 1);
}

#[test]
fn a_bounded_session_is_never_attachable() {
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice").with_class(SessionClass::Bounded));
    assert!(!manager.record(&id).unwrap().is_attachable());
}

#[tokio::test]
async fn resetting_a_session_makes_the_next_turn_start_fresh() {
    let (run, seen) = recording_executor("done", Some("thread-abc"));
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));

    manager.submit(&id, "first").await.unwrap();
    assert!(manager.reset(&id), "there was a binding to drop");
    manager.submit(&id, "second").await.unwrap();

    let calls = seen.lock().unwrap().clone();
    assert_eq!(
        calls[1].1, None,
        "after a reset the next turn carries no context"
    );
}

#[tokio::test]
async fn a_closed_session_refuses_further_turns_and_can_be_forgotten() {
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));

    manager.close(&id).await;
    assert_eq!(manager.record(&id).unwrap().phase, SessionPhase::Closed);
    assert!(manager.submit(&id, "hello").await.is_err());

    assert!(manager.forget(&id), "a closed session can be dropped");
    assert!(manager.record(&id).is_none());
}

#[test]
fn a_live_session_cannot_be_forgotten() {
    // Dropping the record while a process is up would orphan the child.
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));
    assert!(!manager.forget(&id));
    assert!(manager.record(&id).is_some());
}

#[test]
fn observing_an_envelope_creates_the_session_on_first_sight() {
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let envelope = prompt_envelope("wrap-1", "harn-1", "hello");
    let Folded::Observe(observation) = crate::sessions::input::fold_envelope(&envelope) else {
        panic!("expected an observation");
    };
    manager.observe(&observation);

    let records = manager.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].driver, SessionDriver::Envelope);
    assert_eq!(records[0].class, SessionClass::Unbound);
    assert_eq!(records[0].harness_session_id.as_deref(), Some("harn-1"));
    assert_eq!(records[0].workspace, "/repo");
}

#[test]
fn interrupting_an_idle_session_is_a_no_op() {
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));
    assert!(
        !manager.interrupt(&id),
        "there is no turn in flight to interrupt"
    );
}

#[tokio::test]
async fn operations_on_an_unknown_session_are_safe_no_ops() {
    // Every id-keyed entry point must tolerate an id it has never seen.
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);

    assert!(manager.record("ghost").is_none());
    assert!(manager.transcript("ghost").is_empty());
    assert!(!manager.interrupt("ghost"), "nothing to interrupt");
    assert!(!manager.reset("ghost"), "nothing to reset");
    assert!(!manager.forget("ghost"), "nothing to forget");
    manager.close("ghost").await; // must not panic
    assert!(
        manager.submit("ghost", "hi").await.is_err(),
        "no phantom turn"
    );
}

#[test]
fn observing_the_same_session_again_advances_it_without_clobbering_its_id() {
    // The second observation must update the existing row — advancing the turn
    // counter, recording the error — and an empty harness id must never erase
    // the one already captured.
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let key = SessionKey::new("wrap-9", HarnessProvider::Codex);
    let observation = |detail: &str, harness: &str, ends_turn: bool, is_error: bool| Observation {
        key: key.clone(),
        harness_session_id: harness.to_string(),
        cwd: "/repo".to_string(),
        detail: detail.to_string(),
        ends_turn,
        is_error,
        seq: 1,
    };

    manager.observe(&observation("started", "harn-1", false, false));
    manager.observe(&observation("boom", "", true, true));

    assert_eq!(manager.records().len(), 1, "updated a row, not added one");
    let record = manager.records().into_iter().next().unwrap();
    assert_eq!(record.turns, 1, "ends_turn advances the counter");
    assert_eq!(
        record.harness_session_id.as_deref(),
        Some("harn-1"),
        "an empty harness id must not erase a captured one"
    );
    assert_eq!(record.last_error.as_deref(), Some("boom"));
    let role = manager.transcript(&record.id).last().unwrap().role;
    assert_eq!(role, TranscriptRole::Error, "an error renders as an error");
}

#[test]
fn resetting_a_session_with_no_binding_reports_nothing_to_reset() {
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));

    assert!(
        !manager.reset(&id),
        "a fresh session has no binding to drop"
    );
    let note = manager.transcript(&id).last().unwrap().text.clone();
    assert!(
        note.contains("no bound context"),
        "told it was a no-op: {note}"
    );
}

#[tokio::test]
async fn subscribers_are_pinged_on_every_mutation() {
    // The Sessions tab redraws off this ping rather than polling, so a mutation
    // that fires none would leave the UI stale.
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let mut rx = manager.subscribe();

    let id = manager.open(OpenSession::operator("alice"));
    assert!(rx.try_recv().is_ok(), "opening a session must ping");
    // Drain any coalesced pings, then confirm a later mutation pings afresh.
    while rx.try_recv().is_ok() {}
    manager.reset(&id);
    assert!(rx.try_recv().is_ok(), "a reset must ping too");
}

#[test]
fn a_transcript_never_grows_past_its_cap() {
    // push_line is a bounded ring: past the cap the oldest lines are dropped, so
    // a long-lived session cannot grow its transcript without bound.
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    let key = SessionKey::new("chatty", HarnessProvider::Codex);
    let line = |n: usize| Observation {
        key: key.clone(),
        harness_session_id: String::new(),
        cwd: "/repo".to_string(),
        detail: format!("line {n}"),
        ends_turn: false,
        is_error: false,
        seq: n as i64,
    };
    for n in 0..600 {
        manager.observe(&line(n));
    }

    let id = manager.records()[0].id.clone();
    let transcript = manager.transcript(&id);
    assert_eq!(transcript.len(), 500, "the ring is capped");
    assert_eq!(
        transcript.first().unwrap().text,
        "line 100",
        "the oldest overflow lines are evicted"
    );
    assert_eq!(transcript.last().unwrap().text, "line 599");
}

#[test]
fn transport_reflects_both_class_and_provider() {
    let (run, _) = recording_executor("ok", None);
    let manager = manager(run);
    // codex cannot be driven interactively, so even unbound it runs one-shot.
    let codex = manager.open(OpenSession::operator("a"));
    assert_eq!(manager.transport(&codex), Some(Transport::OneShot));
    // an unbound claude session gets the live interactive transport.
    let claude = manager.open(OpenSession::operator("c").with_provider(HarnessProvider::Claude));
    assert_eq!(manager.transport(&claude), Some(Transport::Interactive));
    assert_eq!(manager.transport("ghost"), None, "no session, no transport");
}

#[test]
fn transcript_role_and_open_session_builders_expose_stable_projections() {
    // types.rs projections: role display strings/colors and the OpenSession
    // builder setters.
    use TranscriptRole::{Agent, Error, Status, Tool, User};
    let roles = [User, Agent, Tool, Status, Error];
    let names: Vec<_> = roles.iter().map(|role| role.as_str()).collect();
    assert_eq!(names, ["user", "agent", "tool", "status", "error"]);
    let colors: Vec<_> = roles.iter().map(|role| role.color()).collect();
    assert_eq!(colors, ["cyan", "green", "magenta", "blue", "red"]);

    let request = OpenSession::operator("alice")
        .with_provider(HarnessProvider::Codex)
        .with_driver(SessionDriver::Envelope)
        .with_class(SessionClass::Bounded);
    assert_eq!(request.provider, Some(HarnessProvider::Codex));
    assert_eq!(request.driver, SessionDriver::Envelope);
    assert_eq!(request.class, Some(SessionClass::Bounded));
}
