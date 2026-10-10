//! Unit tests for the progress fold and the idle watchdog.
//!
//! Both are deliberately testable without an inference endpoint:
//! [`super::types::ProgressFold`] is a pure fold over an enum the harness crate
//! already defines, and [`super::watchdog::drive`] is generic over the future it
//! supervises. Every timing test runs on a paused clock, so nothing here sleeps
//! in wall time.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::super::super::types::Abort;
use openhuman_embed::agent_progress::AgentProgress;

use super::types::ProgressFold;
use super::watchdog::drive;
use super::EventSink;

/// The `(kind, payload)` pairs a test sink recorded, shared with the assertion.
type EventLog = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

/// A sink that records `(kind, payload)` for assertions.
fn recording_sink() -> (EventSink, EventLog) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&log);
    let sink = EventSink::new(Some(Box::new(move |event| {
        captured
            .lock()
            .expect("event log poisoned")
            .push((event.event.kind.clone(), event.event.payload.clone()));
    })));
    (sink, log)
}

/// A tool call event with the least interesting fields, for flush boundaries.
fn a_tool_call() -> AgentProgress {
    tool_start("call-9")
}
fn tool_start(id: &str) -> AgentProgress {
    AgentProgress::ToolCallStarted {
        call_id: id.into(),
        tool_name: "shell".into(),
        arguments: serde_json::Value::Null,
        iteration: 1,
        display_label: None,
        display_detail: None,
    }
}
fn text(delta: &str) -> AgentProgress {
    AgentProgress::TextDelta {
        delta: delta.into(),
        iteration: 1,
    }
}
fn thinking(delta: &str) -> AgentProgress {
    AgentProgress::ThinkingDelta {
        delta: delta.into(),
        iteration: 1,
    }
}
fn tool_done(output: &str, error: Option<&str>) -> AgentProgress {
    AgentProgress::ToolCallCompleted {
        call_id: "call-1".into(),
        tool_name: "shell".into(),
        success: error.is_none(),
        output_chars: output.chars().count(),
        output: output.into(),
        arguments: None,
        elapsed_ms: 90,
        iteration: 1,
        failure: None,
        display_label: None,
        display_detail: None,
        structured: None,
    }
}
fn run_done() -> AgentProgress {
    AgentProgress::turn_completed(1)
}
fn telemetry() -> AgentProgress {
    AgentProgress::TurnContent {
        input: None,
        output: None,
    }
}

/// The single event one fresh fold step completes.
fn only_event(fold: &mut ProgressFold, progress: &AgentProgress) -> (String, serde_json::Value) {
    let mut mapped = fold.fold(progress);
    assert_eq!(mapped.len(), 1, "expected exactly one event: {mapped:?}");
    mapped.remove(0)
}

/// `TurnCompleted` drops the pending text instead of flushing it: the executor
/// re-emits the completed reply from the core's return value as its own closing
/// `agent_message`, so flushing here would double the turn's final words.
/// Thinking is *not* dropped with it — the answer's reasoning is recorded.
#[test]
fn run_completed_clears_text_but_still_flushes_thinking() {
    let mut fold = ProgressFold::default();
    fold.fold(&text("the final reply"));
    fold.fold(&thinking("its reasoning"));
    let events = fold.fold(&run_done());
    assert_eq!(
        events.len(),
        2,
        "status + the reasoning snapshot, no doubled message"
    );
    assert_eq!(events[0].0, "agent_thinking");
    assert_eq!(events[0].1["text"], "its reasoning");
    assert_eq!(events[1].0, "status");
    assert_eq!(events[1].1["state"], "idle");
}

#[test]
fn run_and_model_boundaries_fold_to_status() {
    let mut fold = ProgressFold::default();
    let (kind, payload) = only_event(&mut fold, &AgentProgress::TurnStarted);
    assert_eq!(kind, "status");
    assert_eq!(payload["state"], "running");
    assert_eq!(payload["detail"], "turn started");

    let (kind, payload) = only_event(
        &mut fold,
        &AgentProgress::IterationStarted {
            iteration: 1,
            max_iterations: 40,
        },
    );
    assert_eq!(kind, "status");
    assert_eq!(payload["detail"], "model iteration 1");

    let (kind, payload) = only_event(&mut fold, &run_done());
    assert_eq!(kind, "status");
    assert_eq!(payload["state"], "idle");
    assert_eq!(payload["detail"], "turn completed");
}

/// A single delta is a fragment, not a completed message: emitting it would
/// feed the transcript one entry per token and exhaust its cap mid-turn.
#[test]
fn a_lone_text_delta_completes_nothing() {
    let mut fold = ProgressFold::default();
    let events = fold.fold(&text("hello"));
    assert!(
        events.is_empty(),
        "a delta is not a message yet: {events:?}"
    );
}

/// Streamed text becomes one whole message at the next structural boundary.
#[test]
fn text_deltas_coalesce_into_one_message_at_a_boundary() {
    let mut fold = ProgressFold::default();
    fold.fold(&text("the "));
    fold.fold(&text("answer"));
    let events = fold.fold(&a_tool_call());
    assert_eq!(events.len(), 2, "one coalesced message + the tool call");
    assert_eq!(events[0].0, "agent_message");
    assert_eq!(events[0].1["text"], "the answer");
    assert_eq!(events[1].0, "tool_call");
}

/// Reasoning accumulates the same way: one `agent_thinking` per completed
/// block, carrying the whole snapshot rather than a per-token fragment — what
/// the status throttler treats as the reasoning so far.
#[test]
fn thinking_deltas_accumulate_into_one_snapshot_at_a_boundary() {
    let mut fold = ProgressFold::default();
    fold.fold(&thinking("hmm, "));
    fold.fold(&thinking("maybe"));
    let events = fold.fold(&a_tool_call());
    assert_eq!(events[0].0, "agent_thinking");
    assert_eq!(events[0].1["text"], "hmm, maybe");
    assert_eq!(events.len(), 2);
}

/// One delta may carry both fragments, and each must land in its own buffer —
/// merging them would put the model's scratch work inside its answer.
#[test]
fn a_delta_carrying_both_fragments_splits_them() {
    let mut fold = ProgressFold::default();
    fold.fold(&text("visible"));
    fold.fold(&thinking("hidden"));
    let events = fold.fold(&a_tool_call());
    let by_kind: std::collections::HashMap<_, _> = events
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    assert_eq!(by_kind["agent_message"]["text"], "visible");
    assert_eq!(by_kind["agent_thinking"]["text"], "hidden");
}

#[test]
fn a_very_long_thinking_block_is_bounded_to_its_tail() {
    let mut fold = ProgressFold::default();
    fold.fold(&thinking(&"reason ".repeat(600)));
    let events = fold.fold(&a_tool_call());
    let text = events[0].1["text"].as_str().expect("text payload");
    assert!(text.starts_with('…'), "tail elision marker: {text:?}");
    assert!(text.chars().count() <= 780, "snapshot exceeds the bound");
    assert!(
        text.ends_with("reason "),
        "the tail surviving is the newest"
    );
}

/// Telemetry sitting between tokens is not a structural boundary: flushing
/// there would split a message the model has not finished uttering.
#[test]
fn unmapped_events_do_not_split_an_utterance() {
    let mut fold = ProgressFold::default();
    fold.fold(&text("first half"));
    let telemetry = fold.fold(&telemetry());
    assert!(telemetry.is_empty(), "a cache miss carries no stream frame");
    fold.fold(&text(", second half"));
    let events = fold.fold(&a_tool_call());
    assert_eq!(events[0].1["text"], "first half, second half");
}

/// The start event names the tool so a long build shows as running rather than
/// as a gap. The arguments are not on it — the harness carries them on the
/// completion — and claiming an empty object would read as "called with no
/// arguments" rather than "not reported yet".
#[test]
fn a_tool_start_folds_to_a_tool_call_naming_the_tool() {
    let mut fold = ProgressFold::default();
    let (kind, payload) = only_event(&mut fold, &tool_start("call-1"));
    assert_eq!(kind, "tool_call");
    assert_eq!(payload["call_id"], "call-1");
    assert_eq!(payload["tool_name"], "shell");
    assert_eq!(payload["tool_kind"], "other");
    assert_eq!(payload["display"], "shell");
    assert!(payload["input"].is_null());
}

#[test]
fn a_failed_tool_call_folds_to_an_error_flagged_tool_result() {
    let mut fold = ProgressFold::default();
    // Multi-byte output: four characters, six UTF-8 bytes. `output_bytes` is a
    // byte length, so it must report 6 — a status consumer renders the number
    // as bytes.
    let (kind, payload) = only_event(&mut fold, &tool_done("bööm", Some("exit 1")));
    assert_eq!(kind, "tool_result");
    assert_eq!(payload["call_id"], "call-1");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["is_error"], true);
    assert_eq!(payload["output"], "bööm");
    assert_eq!(payload["output_bytes"], 6);
    // In-process tools have no exit status, so none is claimed.
    assert!(payload.get("exit_code").is_none());
}

/// A successful call is not error-flagged, and its captured output reaches the
/// transcript as the text the tool produced — not as a quoted JSON string,
/// which is what rendering the captured `Value` through `to_string` would show.
#[test]
fn a_successful_tool_call_carries_its_output_unquoted() {
    let mut fold = ProgressFold::default();
    let (_, payload) = only_event(&mut fold, &tool_done("exit: 0", None));
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["is_error"], false);
    assert_eq!(payload["output"], "exit: 0");
}

/// Distinct from a completion carrying an error, which was fed back to the
/// model: this means the run is aborting, so it must still produce a terminal
/// row rather than leaving a started call with no end.
#[test]
fn a_tool_failure_folds_to_a_terminal_result() {
    let mut fold = ProgressFold::default();
    let (kind, payload) = only_event(&mut fold, &tool_done("the sky fell", Some("failed")));
    assert_eq!(kind, "tool_result");
    assert_eq!(payload["ok"], false);
    assert!(
        payload["output"].as_str().unwrap().contains("the sky fell"),
        "{payload}"
    );
}

#[test]
fn accounting_only_events_fold_to_nothing() {
    let mut fold = ProgressFold::default();
    let mapped = fold.fold(&telemetry());
    assert!(mapped.is_empty(), "a cache hit carries no stream frame");
}

#[tokio::test(start_paused = true)]
async fn a_working_turn_outlives_the_idle_ceiling() {
    let (tx, mut rx) = mpsc::channel(8);
    let (mut sink, log) = recording_sink();

    // Ten iterations 6s apart: 60s of work under a 10s idle ceiling. The flat
    // wall-clock cap this replaced would have killed it at 10s.
    let call = async move {
        for _ in 1..=10u32 {
            tokio::time::sleep(Duration::from_secs(6)).await;
            tx.send(a_tool_call())
                .await
                .expect("watchdog dropped the receiver");
        }
        "finished"
    };

    let outcome = drive(call, &mut rx, &Abort::new(), 10_000, &mut sink).await;

    assert_eq!(outcome, Ok("finished"));
    assert_eq!(sink.emitted(), 10);
    assert_eq!(log.lock().expect("event log poisoned").len(), 10);
}

#[tokio::test(start_paused = true)]
async fn a_silent_turn_is_timed_out() {
    let (_tx, mut rx) = mpsc::channel::<AgentProgress>(8);
    let (mut sink, _log) = recording_sink();

    let call = async {
        tokio::time::sleep(Duration::from_secs(600)).await;
        "never observed"
    };

    let outcome = drive(call, &mut rx, &Abort::new(), 10_000, &mut sink).await;

    assert_eq!(
        outcome,
        Err("local task idle for 10000ms (no events)".to_string())
    );
    assert_eq!(sink.emitted(), 0);
}

#[tokio::test(start_paused = true)]
async fn silence_after_progress_still_times_out() {
    let (tx, mut rx) = mpsc::channel(8);
    let (mut sink, _log) = recording_sink();

    // Busy for a while, then hangs: the deadline resets while events flow and
    // fires once they stop, which is the whole point of resetting it.
    let call = async move {
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_secs(6)).await;
            tx.send(a_tool_call())
                .await
                .expect("watchdog dropped the receiver");
        }
        tokio::time::sleep(Duration::from_secs(600)).await;
        "never observed"
    };

    let outcome = drive(call, &mut rx, &Abort::new(), 10_000, &mut sink).await;

    assert_eq!(
        outcome,
        Err("local task idle for 10000ms (no events)".to_string())
    );
    assert_eq!(sink.emitted(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_zero_timeout_sets_no_ceiling() {
    let (_tx, mut rx) = mpsc::channel::<AgentProgress>(8);
    let (mut sink, _log) = recording_sink();

    let call = async {
        tokio::time::sleep(Duration::from_secs(86_400)).await;
        "finished"
    };

    let outcome = drive(call, &mut rx, &Abort::new(), 0, &mut sink).await;

    assert_eq!(outcome, Ok("finished"));
}

#[tokio::test(start_paused = true)]
async fn an_abort_stops_the_turn() {
    let (_tx, mut rx) = mpsc::channel::<AgentProgress>(8);
    let (mut sink, _log) = recording_sink();
    let abort = Abort::new();
    abort.abort();

    let call = async {
        tokio::time::sleep(Duration::from_secs(600)).await;
        "never observed"
    };

    let outcome = drive(call, &mut rx, &abort, 10_000, &mut sink).await;

    assert_eq!(outcome, Err("local task aborted".to_string()));
}

#[tokio::test(start_paused = true)]
async fn events_queued_when_the_call_resolves_are_drained() {
    let (tx, mut rx) = mpsc::channel(8);
    let (mut sink, log) = recording_sink();

    // Nothing awaits between the sends and the return, so the call future
    // completes with both deltas and their boundary still sitting in the
    // channel.
    let call = async move {
        tx.send(text("the "))
            .await
            .expect("watchdog dropped the receiver");
        tx.send(text("answer"))
            .await
            .expect("watchdog dropped the receiver");
        tx.send(a_tool_call())
            .await
            .expect("watchdog dropped the receiver");
        "finished"
    };

    let outcome = drive(call, &mut rx, &Abort::new(), 10_000, &mut sink).await;

    assert_eq!(outcome, Ok("finished"));
    // The two deltas complete nothing alone; the iteration boundary emits them
    // as one message beside its own status frame.
    assert_eq!(sink.emitted(), 2);
    let log = log.lock().expect("event log poisoned");
    assert_eq!(log[0].0, "agent_message");
    assert_eq!(log[0].1["text"], "the answer");
    assert_eq!(log[1].0, "tool_call");
}

#[tokio::test(start_paused = true)]
async fn a_closed_progress_channel_does_not_spin_the_loop() {
    let (tx, mut rx) = mpsc::channel::<AgentProgress>(8);
    let (mut sink, _log) = recording_sink();
    drop(tx);

    // With the sender gone `recv` is permanently ready; the watchdog must
    // retire that branch and still honour its deadline rather than livelock.
    let call = async {
        tokio::time::sleep(Duration::from_secs(600)).await;
        "never observed"
    };

    let outcome = drive(call, &mut rx, &Abort::new(), 10_000, &mut sink).await;

    assert_eq!(
        outcome,
        Err("local task idle for 10000ms (no events)".to_string())
    );
}
