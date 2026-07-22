//! Tests for the interactive transport: the pure frame fold and stdin encoders,
//! plus the live process driven by a fake `/bin/sh` harness (spawn, turn loop,
//! interrupt, and teardown) so the plumbing is pinned without a real `claude`.

use serde_json::json;

use super::frames::{encode_interrupt, encode_user_message, map_stream_frame, StreamEvent};
use super::session::{build_interactive_args, InteractiveSession, InteractiveSpec};
use crate::daemon::providers::Abort;
use crate::tinyplace::HarnessProvider;

#[test]
fn a_system_init_frame_announces_the_session_id() {
    let frame = json!({
        "type": "system",
        "subtype": "init",
        "session_id": "abc-123",
    });
    assert_eq!(
        map_stream_frame(&frame),
        vec![StreamEvent::Session {
            session_id: "abc-123".to_string()
        }]
    );
}

#[test]
fn a_non_init_system_frame_announces_nothing() {
    let frame = json!({ "type": "system", "subtype": "other", "session_id": "abc" });
    assert!(map_stream_frame(&frame).is_empty());
}

#[test]
fn a_result_frame_is_the_turn_terminator() {
    // The single most load-bearing shape in the module: completion is this
    // frame, not process exit.
    let frame = json!({
        "type": "result",
        "result": "all done",
        "is_error": false,
        "session_id": "abc-123",
    });
    assert_eq!(
        map_stream_frame(&frame),
        vec![StreamEvent::Result {
            reply: "all done".to_string(),
            is_error: false,
            session_id: Some("abc-123".to_string()),
        }]
    );
}

#[test]
fn an_interrupted_result_frame_still_terminates_the_turn() {
    // What the CLI emits after an in-band interrupt: an error-flagged result
    // with no text. It must still fold to a terminator, or the turn hangs.
    let frame = json!({
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": true,
    });
    assert_eq!(
        map_stream_frame(&frame),
        vec![StreamEvent::Result {
            reply: String::new(),
            is_error: true,
            session_id: None,
        }]
    );
}

#[test]
fn one_assistant_frame_can_carry_several_blocks() {
    let frame = json!({
        "type": "assistant",
        "message": { "content": [
            { "type": "thinking", "thinking": "hmm" },
            { "type": "text", "text": "here" },
            { "type": "tool_use", "name": "Read", "input": { "path": "a.rs" } },
        ]},
    });
    let events = map_stream_frame(&frame);
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        StreamEvent::ReasoningDelta {
            text: "hmm".to_string()
        }
    );
    assert_eq!(
        events[1],
        StreamEvent::AssistantDelta {
            text: "here".to_string()
        }
    );
    assert!(
        matches!(&events[2], StreamEvent::Tool { label } if label.starts_with("Read · ")),
        "got {:?}",
        events[2]
    );
}

#[test]
fn empty_blocks_and_unknown_frame_types_fold_to_nothing() {
    // The stream is an open vocabulary: user echoes and control responses must
    // pass through silently rather than erroring.
    assert!(map_stream_frame(&json!({ "type": "user" })).is_empty());
    assert!(map_stream_frame(&json!({ "type": "control_response" })).is_empty());
    assert!(map_stream_frame(&json!({ "no_type": true })).is_empty());
    assert!(map_stream_frame(&json!({
        "type": "assistant",
        "message": { "content": [{ "type": "text", "text": "" }] },
    }))
    .is_empty());
}

#[test]
fn a_long_tool_input_is_clipped() {
    let frame = json!({
        "type": "assistant",
        "message": { "content": [
            { "type": "tool_use", "name": "Write", "input": { "body": "x".repeat(500) } },
        ]},
    });
    let events = map_stream_frame(&frame);
    let StreamEvent::Tool { label } = &events[0] else {
        panic!("expected a tool event");
    };
    assert!(label.ends_with('…'), "long input must be clipped: {label}");
    assert!(label.chars().count() < 300);
}

#[test]
fn a_turn_is_encoded_as_one_newline_terminated_user_message() {
    let line = encode_user_message("hello");
    assert!(
        line.ends_with('\n'),
        "the CLI reads one JSON object per line"
    );
    assert_eq!(line.matches('\n').count(), 1);
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).expect("valid JSON");
    assert_eq!(parsed["type"], "user");
    assert_eq!(parsed["message"]["role"], "user");
    assert_eq!(parsed["message"]["content"][0]["text"], "hello");
}

#[test]
fn a_multiline_prompt_stays_one_wire_line() {
    // A raw newline in the prompt would split the frame and desync the stream.
    let line = encode_user_message("first\nsecond");
    assert_eq!(line.matches('\n').count(), 1);
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).expect("valid JSON");
    assert_eq!(parsed["message"]["content"][0]["text"], "first\nsecond");
}

#[test]
fn an_interrupt_is_encoded_as_a_control_request() {
    let line = encode_interrupt("req_interrupt_1");
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).expect("valid JSON");
    assert_eq!(parsed["type"], "control_request");
    assert_eq!(parsed["request_id"], "req_interrupt_1");
    assert_eq!(parsed["request"]["subtype"], "interrupt");
}

fn spec() -> InteractiveSpec {
    InteractiveSpec {
        provider: HarnessProvider::Claude,
        bin: "claude".to_string(),
        cwd: "/repo".to_string(),
        env: std::collections::HashMap::new(),
        model: None,
        append_system_prompt: None,
        skip_permissions: false,
        extra_args: Vec::new(),
    }
}

#[test]
fn the_interactive_argv_asks_for_a_stream_json_channel_in_both_directions() {
    let args = build_interactive_args(&spec());
    assert_eq!(
        args,
        vec![
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose"
        ]
    );
}

#[test]
fn the_interactive_argv_carries_no_prompt() {
    // Turns arrive on stdin, which is what sidesteps the leading-dash injection
    // hazard the one-shot argv has to neutralize.
    let mut spec = spec();
    spec.model = Some("opus".to_string());
    spec.skip_permissions = true;
    spec.extra_args = vec!["--foo".to_string()];
    let args = build_interactive_args(&spec);

    assert!(args.contains(&"--model".to_string()));
    assert!(args.contains(&"opus".to_string()));
    assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    assert_eq!(args.last().unwrap(), "--foo");
    assert!(
        !args.iter().any(|arg| arg == "-"),
        "no positional prompt belongs on an interactive argv"
    );
}

#[tokio::test]
async fn open_rejects_a_binary_that_is_not_on_the_sessions_path() {
    // The bin is untrusted config; with an empty PATH nothing resolves.
    let mut spec = spec();
    spec.bin = "definitely-not-a-real-harness".to_string();
    spec.env.insert("PATH".to_string(), String::new());

    let err = InteractiveSession::open(&spec)
        .await
        .err()
        .expect("a missing binary must be refused before any spawn");
    assert!(
        err.contains("is not an executable on the session's PATH"),
        "got: {err}"
    );
}

// Live-process tests: drive a *fake* claude — a shell script that ignores its
// argv and speaks stream-json on stdin/stdout — so spawn, pipes, turn loop, and
// teardown run offline. Unix-only: Windows CI has no `/bin/sh`.

/// Wrap `body` as a `/bin/sh` script in `dir` and build a spec pointing at it.
#[cfg(unix)]
fn fake_harness(dir: &std::path::Path, body: &str) -> InteractiveSpec {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut env = std::collections::HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    InteractiveSpec {
        provider: HarnessProvider::Claude,
        bin: path.to_string_lossy().into_owned(),
        cwd: dir.to_string_lossy().into_owned(),
        env,
        model: None,
        append_system_prompt: None,
        skip_permissions: false,
        extra_args: Vec::new(),
    }
}

/// Answers every turn: announce the session once, emit a line of non-JSON noise
/// (the reader must tolerate it), stream a text block, then a `result`. An
/// interrupt is answered with the error-flagged terminator.
#[cfg(unix)]
const ANSWERING_BODY: &str = r#"
init=1
while IFS= read -r line; do
  case "$line" in
    *control_request*)
      printf '{"type":"result","subtype":"error_during_execution","is_error":true}\n' ;;
    *)
      if [ "$init" = 1 ]; then
        printf '{"type":"system","subtype":"init","session_id":"sess-42"}\n'
        init=0
      fi
      printf 'human-readable noise, not a frame\n'
      printf '{"type":"assistant","message":{"content":[{"type":"text","text":"streamed bit"}]}}\n'
      printf '{"type":"result","result":"final answer","is_error":false,"session_id":"sess-42"}\n'
      ;;
  esac
done
"#;

#[cfg(unix)]
#[tokio::test]
async fn a_turn_streams_its_events_and_settles_on_the_result_frame() {
    use std::sync::{Arc, Mutex};

    let dir = tempfile::tempdir().unwrap();
    let session = InteractiveSession::open(&fake_harness(dir.path(), ANSWERING_BODY))
        .await
        .expect("the fake harness must spawn");

    assert_eq!(
        session.harness_session_id(),
        None,
        "no id is known before the harness announces one"
    );

    let seen: Arc<Mutex<Vec<StreamEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), {
        let seen = seen.clone();
        session.submit("hello", &Abort::new(), move |ev| {
            seen.lock().unwrap().push(ev.clone())
        })
    })
    .await
    .expect("the turn must settle, not hang")
    .expect("the turn must succeed");

    assert_eq!(
        outcome.reply, "final answer",
        "the reply is the result frame's text, not the streamed fragment"
    );
    assert!(!outcome.aborted);
    assert!(!outcome.is_error);
    assert_eq!(outcome.harness_session_id.as_deref(), Some("sess-42"));
    assert_eq!(
        session.harness_session_id().as_deref(),
        Some("sess-42"),
        "the announced id is recorded on the session"
    );

    let events = seen.lock().unwrap().clone();
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::AssistantDelta { text } if text == "streamed bit")));
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::Result { .. })));
    session.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_second_turn_reuses_the_same_live_session() {
    let dir = tempfile::tempdir().unwrap();
    let session = InteractiveSession::open(&fake_harness(dir.path(), ANSWERING_BODY))
        .await
        .expect("spawn");

    for _ in 0..2 {
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            session.submit("again", &Abort::new(), |_| {}),
        )
        .await
        .expect("each turn on the live session must settle")
        .expect("each turn must succeed");
        assert_eq!(outcome.reply, "final answer");
    }
    session.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn an_empty_result_frame_keeps_the_streamed_text() {
    // When the result carries no text, the streamed deltas are the answer.
    let body = r#"
while IFS= read -r line; do
  case "$line" in
    *control_request*) : ;;
    *)
      printf '{"type":"assistant","message":{"content":[{"type":"text","text":"kept text"}]}}\n'
      printf '{"type":"result","result":"","is_error":false}\n'
      ;;
  esac
done
"#;
    let dir = tempfile::tempdir().unwrap();
    let session = InteractiveSession::open(&fake_harness(dir.path(), body))
        .await
        .expect("spawn");
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        session.submit("go", &Abort::new(), |_| {}),
    )
    .await
    .expect("settle")
    .expect("succeed");
    assert_eq!(
        outcome.reply, "kept text",
        "an empty result must fall back to the streamed text"
    );
    session.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn an_abort_interrupts_the_turn_and_settles_it_as_aborted() {
    // The harness streams a fragment then stalls; the in-band interrupt ends it,
    // and the turn settles as aborted, keeping the streamed text.
    let body = r#"
while IFS= read -r line; do
  case "$line" in
    *control_request*)
      printf '{"type":"result","subtype":"error_during_execution","is_error":true}\n' ;;
    *)
      printf '{"type":"assistant","message":{"content":[{"type":"text","text":"partial"}]}}\n'
      ;;
  esac
done
"#;
    let dir = tempfile::tempdir().unwrap();
    let session = InteractiveSession::open(&fake_harness(dir.path(), body))
        .await
        .expect("spawn");
    let abort = Abort::new();
    let handle = tokio::spawn({
        let (session, abort) = (session.clone(), abort.clone());
        async move { session.submit("work", &abort, |_| {}).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    abort.abort();

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), handle)
        .await
        .expect("the interrupt's terminating result must settle the turn promptly")
        .expect("no panic")
        .expect("an interrupted turn is a settled turn, not an error");
    assert!(outcome.aborted, "the turn must be reported as aborted");
    assert_eq!(
        outcome.reply, "partial",
        "an aborted turn keeps its streamed text, not the empty result field"
    );
    session.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn an_ignored_interrupt_settles_through_the_grace_window() {
    // A swallowed interrupt must not hang the turn: the grace timer settles it.
    let body = r#"
while IFS= read -r line; do
  case "$line" in
    *control_request*) : ;;
    *) printf '{"type":"assistant","message":{"content":[{"type":"text","text":"partial"}]}}\n' ;;
  esac
done
"#;
    let dir = tempfile::tempdir().unwrap();
    let session = InteractiveSession::open(&fake_harness(dir.path(), body))
        .await
        .expect("spawn");
    let abort = Abort::new();
    let handle = tokio::spawn({
        let (session, abort) = (session.clone(), abort.clone());
        async move { session.submit("work", &abort, |_| {}).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    abort.abort();

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), handle)
        .await
        .expect("the grace window must settle a swallowed interrupt")
        .expect("no panic")
        .expect("a grace-settled turn is aborted, not an error");
    assert!(outcome.aborted);
    assert!(!outcome.is_error, "a grace fallback is not a harness error");
    session.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn submitting_after_close_is_a_clean_error() {
    let dir = tempfile::tempdir().unwrap();
    let session = InteractiveSession::open(&fake_harness(dir.path(), ANSWERING_BODY))
        .await
        .expect("spawn");
    session.close().await;

    let err = session
        .submit("too late", &Abort::new(), |_| {})
        .await
        .expect_err("a closed session cannot accept a turn");
    assert!(err.contains("interactive session is closed"), "got: {err}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_child_that_exits_before_a_result_errors_rather_than_hanging() {
    // The child reads the prompt then dies with no result. Stream end is not
    // completion — the turn must error, not settle empty.
    let body = "IFS= read -r line";
    let dir = tempfile::tempdir().unwrap();
    let session = InteractiveSession::open(&fake_harness(dir.path(), body))
        .await
        .expect("spawn");
    let err = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        session.submit("go", &Abort::new(), |_| {}),
    )
    .await
    .expect("a dead child must error promptly, not hang")
    .expect_err("a stream that ends before a result is a failed turn");
    assert!(
        err.contains("ended before the turn completed"),
        "got: {err}"
    );
    session.close().await;
}
