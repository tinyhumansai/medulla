//! Turn execution: the bounded/unbound split, capture-and-resume, and failure survival.

use super::*;

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

#[tokio::test]
async fn a_session_mid_turn_rejects_a_second_turn_as_busy() {
    // A turn already in flight is reported, not queued: the operator must see
    // why their second prompt did nothing.
    let (run, release) = gated_executor();
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));
    let background = submit_in_background(&manager, &id, "first");
    wait_for_phase(&manager, &id, SessionPhase::Turn).await;

    let err = manager.submit(&id, "second").await.expect_err("rejected");
    assert!(err.contains("busy"), "the rejection must say why: {err}");

    release.notify_one();
    background.await.unwrap().expect("the first turn completes");
}

#[tokio::test]
async fn interrupting_a_running_turn_moves_it_to_interrupting_then_back_to_live() {
    // An interrupt ends the turn, never the session: the phase passes through
    // Interrupting and settles back on Live so the next turn is accepted.
    let (run, release) = gated_executor();
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));
    let background = submit_in_background(&manager, &id, "go");
    wait_for_phase(&manager, &id, SessionPhase::Turn).await;

    assert!(manager.interrupt(&id), "a running turn is interruptible");
    let phase = manager.record(&id).unwrap().phase;
    assert_eq!(phase, SessionPhase::Interrupting, "interrupt is immediate");

    release.notify_one();
    background.await.unwrap().expect("the turn settles");
    let phase = manager.record(&id).unwrap().phase;
    assert_eq!(phase, SessionPhase::Live, "the session survives its abort");
}

#[tokio::test]
async fn an_interactive_session_whose_binary_is_missing_fails_the_turn_but_survives() {
    // The interactive transport (claude, unbound) spawns a child on first turn.
    // With no PATH the spawn fails; the turn errors but the session stays
    // non-terminal and the failure is recorded.
    let (run, _) = recording_executor("unused", None);
    let manager = claude_manager(run);
    let id = manager.open(OpenSession::operator("alice"));

    let err = manager
        .submit(&id, "go")
        .await
        .expect_err("no claude binary");
    assert!(
        err.contains("claude"),
        "must name the missing binary: {err}"
    );

    let record = manager.record(&id).unwrap();
    assert_eq!(record.turns, 0, "a failed spawn is not a completed turn");
    assert!(record.last_error.is_some(), "the failure is recorded");
    assert!(!record.phase.is_terminal(), "the session stays retryable");
}

#[tokio::test]
async fn a_first_unbound_turn_with_no_open_session_registers_one() {
    // A plain-text DM can drive `run_turn` for a conversation the operator never
    // opened; `ensure_session` must create the row rather than dropping the turn.
    let (run, seen) = recording_executor("hi there", Some("thread-1"));
    let manager = manager(run);
    let key = SessionKey::new("stranger", HarnessProvider::Codex);

    manager
        .run_turn(TurnRequest {
            key: key.clone(),
            class: SessionClass::Unbound,
            text: "hello".to_string(),
            origin: TurnOrigin::Operator,
            model: None,
        })
        .await
        .expect("the turn runs");

    let records = manager.records();
    assert_eq!(records.len(), 1, "the turn must have opened a session");
    assert_eq!(records[0].key, key);
    assert_eq!(records[0].class, SessionClass::Unbound);
    assert_eq!(records[0].turns, 1);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

// ---- interactive transport: the live claude path driven by a fake harness ----
//
// Unix-only: these spawn a `/bin/sh` script standing in for `claude`, so the
// whole manager path (spawn → stream → settle → teardown) runs offline.

/// Answers a turn richly: announce the session, emit a blank line and an
/// oversized (>1 MiB) record the reader must drop, three assistant frames the
/// frame-fold treats as no-content / unknown-block / input-less-tool, then a
/// streamed text block and the terminating `result`. An interrupt is answered
/// with the error-flagged terminator.
#[cfg(unix)]
const RICH_ANSWER: &str = r#"
init=1
while IFS= read -r line; do
  case "$line" in
    *control_request*)
      printf '{"type":"result","subtype":"error_during_execution","is_error":true}\n' ;;
    *)
      if [ "$init" = 1 ]; then
        printf '{"type":"system","subtype":"init","session_id":"sess-1"}\n'
        init=0
      fi
      printf '\n'
      printf '%*s\n' 1100000 ''
      printf '{"type":"assistant","message":{}}\n'
      printf '{"type":"assistant","message":{"content":[{"type":"widget"}]}}\n'
      printf '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read"}]}}\n'
      printf '{"type":"assistant","message":{"content":[{"type":"text","text":"streamed"}]}}\n'
      printf '{"type":"result","result":"final answer","is_error":false,"session_id":"sess-1"}\n'
      ;;
  esac
done
"#;

#[cfg(unix)]
#[tokio::test]
async fn an_interactive_turn_spawns_streams_and_settles_on_the_result() {
    let (manager, _dir) = claude_harness_manager(RICH_ANSWER);
    let id = manager.open(OpenSession::operator("alice"));
    assert_eq!(
        manager.transport(&id),
        Some(Transport::Interactive),
        "an unbound claude session takes the live transport"
    );

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        manager.submit(&id, "hello"),
    )
    .await
    .expect("the interactive turn must settle, not hang")
    .expect("the turn succeeds");

    assert_eq!(
        outcome.reply, "final answer",
        "the reply is the result frame, not the streamed fragment"
    );
    assert!(!outcome.is_error);
    assert_eq!(outcome.harness_session_id.as_deref(), Some("sess-1"));

    let record = manager.record(&id).unwrap();
    assert_eq!(record.turns, 1);
    assert_eq!(
        record.phase,
        SessionPhase::Live,
        "the session stays live for the next turn"
    );
    assert_eq!(record.harness_session_id.as_deref(), Some("sess-1"));

    let transcript = manager.transcript(&id);
    assert!(
        transcript
            .iter()
            .any(|line| line.role == TranscriptRole::Status
                && line.text.contains("harness session sess-1")),
        "the announced session id reaches the transcript: {transcript:?}"
    );
    assert!(
        transcript
            .iter()
            .any(|line| line.role == TranscriptRole::Tool && line.text.starts_with("Read")),
        "the tool call streams into the transcript: {transcript:?}"
    );
    assert!(
        transcript
            .iter()
            .any(|line| line.role == TranscriptRole::Agent && line.text == "final answer"),
        "the answer is recorded: {transcript:?}"
    );

    manager.close(&id).await;
    assert_eq!(manager.record(&id).unwrap().phase, SessionPhase::Closed);
}

/// Answers every turn with an error-flagged `result`.
#[cfg(unix)]
const ERROR_ANSWER: &str = r#"
while IFS= read -r line; do
  case "$line" in
    *control_request*) : ;;
    *) printf '{"type":"result","result":"it broke","is_error":true,"session_id":"s2"}\n' ;;
  esac
done
"#;

#[cfg(unix)]
#[tokio::test]
async fn an_interactive_turn_that_errors_records_it_but_keeps_the_session_live() {
    let (manager, _dir) = claude_harness_manager(ERROR_ANSWER);
    let id = manager.open(OpenSession::operator("alice"));

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        manager.submit(&id, "go"),
    )
    .await
    .expect("settle")
    .expect("an error outcome is still a settled turn, not a transport error");
    assert!(outcome.is_error, "the harness flagged the turn as an error");

    let record = manager.record(&id).unwrap();
    assert_eq!(
        record.last_error.as_deref(),
        Some("it broke"),
        "a harness error is recorded on the session"
    );
    assert_eq!(
        record.phase,
        SessionPhase::Live,
        "an error ends the turn, not the session"
    );
    assert_eq!(record.turns, 1, "a settled error is still a completed turn");
    let last = manager.transcript(&id).into_iter().last().unwrap();
    assert_eq!(
        last.role,
        TranscriptRole::Error,
        "the error renders as an error line"
    );
    assert_eq!(last.text, "it broke");
}

/// Streams a fragment then waits; an in-band interrupt ends the turn.
#[cfg(unix)]
const STALLING_ANSWER: &str = r#"
while IFS= read -r line; do
  case "$line" in
    *control_request*)
      printf '{"type":"result","subtype":"error_during_execution","is_error":true}\n' ;;
    *)
      printf '{"type":"assistant","message":{"content":[{"type":"text","text":"partial"}]}}\n' ;;
  esac
done
"#;

#[cfg(unix)]
#[tokio::test]
async fn interrupting_a_live_interactive_turn_settles_it_aborted_and_stays_live() {
    let (manager, _dir) = claude_harness_manager(STALLING_ANSWER);
    let id = manager.open(OpenSession::operator("alice"));
    let background = submit_in_background(&manager, &id, "work");
    wait_for_phase(&manager, &id, SessionPhase::Turn).await;
    // Let the child stream its fragment before the interrupt lands.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    assert!(manager.interrupt(&id), "a running turn is interruptible");

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), background)
        .await
        .expect("the interrupt's terminator must settle the turn promptly")
        .expect("no panic")
        .expect("an interrupted turn is a settled turn");
    assert!(outcome.aborted, "the turn is reported as aborted");

    let record = manager.record(&id).unwrap();
    assert_eq!(
        record.phase,
        SessionPhase::Live,
        "the session survives its own interrupt"
    );
    let transcript = manager.transcript(&id);
    assert!(
        transcript.iter().any(|line| line
            .text
            .contains("turn interrupted — the session is still live")),
        "the interrupt is noted in the transcript: {transcript:?}"
    );
}

/// Answers each turn, then — once stdin closes — wedges instead of exiting, so
/// [`InteractiveSession::close`] must fall through its reap timeout to a kill.
#[cfg(unix)]
const WEDGES_ON_CLOSE: &str = r#"
while IFS= read -r line; do
  printf '{"type":"result","result":"ok","is_error":false}\n'
done
sleep 30
"#;

#[cfg(unix)]
#[tokio::test]
async fn closing_a_wedged_interactive_child_falls_back_to_a_kill() {
    // The child answers the turn but ignores stdin's EOF and sleeps; close drops
    // stdin, waits out its reap grace, then kills — the session still ends up
    // Closed rather than leaking the process.
    let (manager, _dir) = claude_harness_manager(WEDGES_ON_CLOSE);
    let id = manager.open(OpenSession::operator("alice"));
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        manager.submit(&id, "go"),
    )
    .await
    .expect("settle")
    .expect("succeed");

    tokio::time::timeout(std::time::Duration::from_secs(30), manager.close(&id))
        .await
        .expect("close must not hang even when the child wedges");
    assert_eq!(manager.record(&id).unwrap().phase, SessionPhase::Closed);
}

#[tokio::test]
async fn a_blank_reply_adds_no_agent_line_but_still_counts_the_turn() {
    // push_line drops empty text, so a whitespace-only reply leaves no phantom
    // agent line — yet the turn still completed.
    let (run, _) = recording_executor("   ", None);
    let manager = manager(run);
    let id = manager.open(OpenSession::operator("alice"));
    manager.submit(&id, "go").await.unwrap();

    let lines = manager.transcript(&id);
    assert_eq!(lines.len(), 1, "only the user line survives a blank reply");
    assert_eq!(lines[0].role, TranscriptRole::User);
    let turns = manager.record(&id).unwrap().turns;
    assert_eq!(turns, 1, "a blank reply is still a completed turn");
}
