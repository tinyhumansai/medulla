//! Manager tests: the session lifecycle, the bounded/unbound turn split, and
//! the transcript the Sessions tab renders.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::daemon::providers::{RunTaskFn, RunTaskResult};
use crate::tinyplace::HarnessProvider;

use super::super::input::Folded;
use super::super::manager::{OpenSession, SessionConfig, SessionManager, TranscriptRole};
use super::super::types::{
    SessionClass, SessionDriver, SessionKey, SessionPhase, TurnOrigin, TurnRequest,
};
use super::input_tests::prompt_envelope;

// ---------------------------------------------------------------- manager ---

/// A clock that advances a fixed step on every read, so ordering is observable.
fn stub_clock() -> crate::sessions::manager::NowFn {
    let counter = Arc::new(AtomicI64::new(1_000));
    Arc::new(move || counter.fetch_add(1, Ordering::SeqCst))
}

/// An executor that records the prompts and resume ids it was handed.
#[allow(clippy::type_complexity)]
fn recording_executor(
    reply: &str,
    session_id: Option<&str>,
) -> (RunTaskFn, Arc<Mutex<Vec<(String, Option<String>)>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reply = reply.to_string();
    let session_id = session_id.map(str::to_string);
    let run: RunTaskFn = {
        let seen = seen.clone();
        Arc::new(move |options| {
            seen.lock()
                .unwrap()
                .push((options.prompt.clone(), options.resume_session_id.clone()));
            let reply = reply.clone();
            let session_id = session_id.clone();
            let provider = options.provider;
            Box::pin(async move {
                Ok(RunTaskResult {
                    provider,
                    reply,
                    events: 1,
                    usage: None,
                    session_id,
                })
            })
        })
    };
    (run, seen)
}

fn manager(run: RunTaskFn) -> SessionManager {
    SessionManager::new(
        SessionConfig {
            // codex routes unbound sessions onto the one-shot transport, which
            // is what lets these tests exercise continuity without a real CLI.
            default_provider: HarnessProvider::Codex,
            ..SessionConfig::default()
        },
        run,
    )
    .with_now(stub_clock())
}

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
async fn an_unbound_turn_captures_then_resumes_the_harness_session() {
    let (run, seen) = recording_executor("done", Some("thread-abc"));
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));

    manager.submit(&id, "first").await.expect("first turn runs");
    manager
        .submit(&id, "second")
        .await
        .expect("second turn runs");

    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0],
        ("first".to_string(), None),
        "nothing to resume yet"
    );
    assert_eq!(
        calls[1],
        ("second".to_string(), Some("thread-abc".to_string())),
        "the second turn must resume what the first captured"
    );

    let record = manager.record(&id).unwrap();
    assert_eq!(record.turns, 2);
    assert_eq!(record.phase, SessionPhase::Live);
    assert_eq!(record.harness_session_id.as_deref(), Some("thread-abc"));
}

#[tokio::test]
async fn a_bounded_turn_neither_resumes_nor_binds() {
    let (run, seen) = recording_executor("done", Some("thread-abc"));
    let manager = manager(run);
    let key = SessionKey::new("alice", HarnessProvider::Codex);

    for text in ["one", "two"] {
        manager
            .run_turn(TurnRequest {
                key: key.clone(),
                class: SessionClass::Bounded,
                text: text.to_string(),
                origin: TurnOrigin::Frame {
                    task_id: "t".to_string(),
                    correlation_id: None,
                },
                model: None,
            })
            .await
            .expect("bounded turns run");
    }

    let calls = seen.lock().unwrap().clone();
    assert!(
        calls.iter().all(|(_, resume)| resume.is_none()),
        "two tasks must never see each other's context: {calls:?}"
    );
    assert!(
        manager.registry().is_empty(),
        "a bounded turn must not leave a binding"
    );
    assert!(
        manager.records().is_empty(),
        "a bounded turn must leave no session behind"
    );
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
async fn a_turn_records_both_sides_in_the_transcript() {
    let (run, _) = recording_executor("here you go", None);
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));
    manager.submit(&id, "do the thing").await.unwrap();

    let lines = manager.transcript(&id);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].role, TranscriptRole::User);
    assert_eq!(lines[0].text, "do the thing");
    assert_eq!(lines[1].role, TranscriptRole::Agent);
    assert_eq!(lines[1].text, "here you go");
    assert!(lines[0].at < lines[1].at, "the clock must advance");
}

#[tokio::test]
async fn a_failed_turn_keeps_the_session_alive() {
    let run: RunTaskFn = Arc::new(|_| Box::pin(async { Err("provider exploded".to_string()) }));
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));

    let outcome = manager.submit(&id, "go").await;
    assert!(outcome.is_err());

    let record = manager.record(&id).unwrap();
    assert_eq!(
        record.phase,
        SessionPhase::Live,
        "one bad turn must not kill the conversation"
    );
    assert_eq!(record.last_error.as_deref(), Some("provider exploded"));
    assert_eq!(record.turns, 0, "a failed turn is not a completed turn");
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
async fn an_operator_opened_bounded_session_records_its_one_turn_then_closes() {
    // A bounded turn from a *frame* leaves no record. A bounded session the
    // operator opened has a row on screen, so its turn must appear in it —
    // otherwise the turn reads as having silently done nothing.
    let (run, _) = recording_executor("done", None);
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("one-shot").with_class(SessionClass::Bounded));

    manager
        .submit(&id, "do it once")
        .await
        .expect("the turn runs");

    let lines = manager.transcript(&id);
    assert_eq!(lines[0].role, TranscriptRole::User);
    assert_eq!(lines[0].text, "do it once");
    assert_eq!(lines[1].role, TranscriptRole::Agent);
    assert_eq!(lines[1].text, "done");

    let record = manager.record(&id).unwrap();
    assert_eq!(record.turns, 1);
    assert_eq!(
        record.phase,
        SessionPhase::Closed,
        "one turn, then gone — that is what bounded means"
    );
    assert!(
        manager.submit(&id, "again").await.is_err(),
        "a spent bounded session takes no second turn"
    );
    assert!(manager.forget(&id), "and can then be dropped");
}

#[tokio::test]
async fn a_bounded_session_leaves_no_binding_even_when_operator_opened() {
    let (run, seen) = recording_executor("done", Some("thread-abc"));
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("one-shot").with_class(SessionClass::Bounded));
    manager.submit(&id, "go").await.unwrap();

    assert_eq!(seen.lock().unwrap()[0].1, None, "nothing to resume");
    assert!(
        manager.registry().is_empty(),
        "a bounded turn must never bind, however it was started"
    );
}
