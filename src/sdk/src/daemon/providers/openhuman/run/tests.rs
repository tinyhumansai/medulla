//! Unit tests for the progress fold and the idle watchdog.
//!
//! Both are deliberately testable without a core: [`super::types::ProgressFold`]
//! is a pure fold over an enum the vendored crate already defines, and
//! [`super::watchdog::drive`] is generic over the future it supervises. Every
//! timing test runs on a paused clock, so nothing here sleeps in wall time.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use super::super::super::types::Abort;
use super::core_contract::AgentProgress;
use super::types::{EventSink, ProgressFold};
use super::watchdog::drive;

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
    AgentProgress::ToolCallStarted {
        call_id: "call-9".to_string(),
        tool_name: "bash".to_string(),
        arguments: json!({}),
        iteration: 1,
        display_label: None,
        display_detail: None,
    }
}

/// The single event one fresh fold step completes.
fn only_event(fold: &mut ProgressFold, progress: &AgentProgress) -> (String, serde_json::Value) {
    let mut mapped = fold.fold(progress);
    assert_eq!(mapped.len(), 1, "expected exactly one event: {mapped:?}");
    mapped.remove(0)
}

#[test]
fn turn_and_iteration_boundaries_fold_to_status() {
    let mut fold = ProgressFold::default();
    let (kind, payload) = only_event(&mut fold, &AgentProgress::TurnStarted);
    assert_eq!(kind, "status");
    assert_eq!(payload["state"], "running");
    assert_eq!(payload["detail"], "turn started");

    let (kind, payload) = only_event(
        &mut fold,
        &AgentProgress::IterationStarted {
            iteration: 3,
            max_iterations: 40,
        },
    );
    assert_eq!(kind, "status");
    assert_eq!(payload["detail"], "iteration 3/40");

    let (kind, payload) = only_event(&mut fold, &AgentProgress::TurnCompleted { iterations: 7 });
    assert_eq!(kind, "status");
    assert_eq!(payload["state"], "idle");
    assert_eq!(payload["detail"], "turn completed");
}

/// A single delta is a fragment, not a completed message: emitting it would
/// feed the transcript one entry per token and exhaust its cap mid-turn.
#[test]
fn a_lone_text_delta_completes_nothing() {
    let mut fold = ProgressFold::default();
    let events = fold.fold(&AgentProgress::TextDelta {
        delta: "hello".to_string(),
        iteration: 1,
    });
    assert!(
        events.is_empty(),
        "a delta is not a message yet: {events:?}"
    );
}

/// Streamed text becomes one whole message at the next phase boundary.
#[test]
fn text_deltas_coalesce_into_one_message_at_a_boundary() {
    let mut fold = ProgressFold::default();
    fold.fold(&AgentProgress::TextDelta {
        delta: "the ".to_string(),
        iteration: 1,
    });
    fold.fold(&AgentProgress::TextDelta {
        delta: "answer".to_string(),
        iteration: 1,
    });
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
    fold.fold(&AgentProgress::ThinkingDelta {
        delta: "hmm, ".to_string(),
        iteration: 1,
    });
    fold.fold(&AgentProgress::ThinkingDelta {
        delta: "maybe".to_string(),
        iteration: 1,
    });
    let events = fold.fold(&a_tool_call());
    assert_eq!(events[0].0, "agent_thinking");
    assert_eq!(events[0].1["text"], "hmm, maybe");
    assert_eq!(events.len(), 2);
}

#[test]
fn a_very_long_thinking_block_is_bounded_to_its_tail() {
    let mut fold = ProgressFold::default();
    fold.fold(&AgentProgress::ThinkingDelta {
        delta: "reason ".repeat(600),
        iteration: 1,
    });
    let events = fold.fold(&a_tool_call());
    let text = events[0].1["text"].as_str().expect("text payload");
    assert!(text.starts_with('…'), "tail elision marker: {text:?}");
    assert!(text.chars().count() <= 780, "snapshot exceeds the bound");
    assert!(
        text.ends_with("reason "),
        "the tail surviving is the newest"
    );
}

/// Telemetry sitting between tokens is not a phase boundary: flushing there
/// would split a message the model has not finished uttering.
#[test]
fn cost_rollups_do_not_split_an_utterance() {
    let mut fold = ProgressFold::default();
    fold.fold(&AgentProgress::TextDelta {
        delta: "first half".to_string(),
        iteration: 1,
    });
    let telemetry = fold.fold(&AgentProgress::TurnCostUpdated {
        model: "chat-v1".to_string(),
        iteration: 1,
        input_tokens: 10,
        output_tokens: 20,
        cached_input_tokens: 0,
        total_usd: 0.01,
    });
    assert!(telemetry.is_empty(), "cost rollups carry no stream frame");
    fold.fold(&AgentProgress::TextDelta {
        delta: ", second half".to_string(),
        iteration: 1,
    });
    let events = fold.fold(&a_tool_call());
    assert_eq!(events[0].1["text"], "first half, second half");
}

#[test]
fn tool_call_started_folds_to_a_tool_call_carrying_its_arguments() {
    let mut fold = ProgressFold::default();
    let (kind, payload) = only_event(
        &mut fold,
        &AgentProgress::ToolCallStarted {
            call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            arguments: json!({ "command": "cargo test" }),
            iteration: 2,
            display_label: Some("Running tests".to_string()),
            display_detail: None,
        },
    );
    assert_eq!(kind, "tool_call");
    assert_eq!(payload["call_id"], "call-1");
    assert_eq!(payload["tool_name"], "bash");
    assert_eq!(payload["tool_kind"], "other");
    assert_eq!(payload["display"], "Running tests");
    assert_eq!(payload["input"]["command"], "cargo test");
}

#[test]
fn a_tool_call_without_a_label_displays_its_tool_name() {
    let mut fold = ProgressFold::default();
    let (_, payload) = only_event(
        &mut fold,
        &AgentProgress::ToolCallStarted {
            call_id: "call-2".to_string(),
            tool_name: "read_file".to_string(),
            arguments: json!({}),
            iteration: 1,
            display_label: None,
            display_detail: None,
        },
    );
    assert_eq!(payload["display"], "read_file");
}

#[test]
fn a_failed_tool_call_folds_to_an_error_flagged_tool_result() {
    let mut fold = ProgressFold::default();
    // Multi-byte output: four characters, six UTF-8 bytes. `output_bytes` is a
    // byte length, so it must report 6, not the core's character count of 4 —
    // a status consumer renders the number as bytes.
    let (kind, payload) = only_event(
        &mut fold,
        &AgentProgress::ToolCallCompleted {
            call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            success: false,
            output_chars: 4,
            output: "bööm".to_string(),
            arguments: None,
            elapsed_ms: 90,
            iteration: 2,
            failure: None,
        },
    );
    assert_eq!(kind, "tool_result");
    assert_eq!(payload["call_id"], "call-1");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["is_error"], true);
    assert_eq!(payload["output"], "bööm");
    assert_eq!(payload["output_bytes"], 6);
    // In-process tools have no exit status, so none is claimed.
    assert!(payload.get("exit_code").is_none());
}

#[test]
fn accounting_only_progress_folds_to_nothing() {
    let mut fold = ProgressFold::default();
    let mapped = fold.fold(&AgentProgress::TurnCostUpdated {
        model: "chat-v1".to_string(),
        iteration: 1,
        input_tokens: 10,
        output_tokens: 20,
        cached_input_tokens: 0,
        total_usd: 0.01,
    });
    assert!(mapped.is_empty(), "cost rollups carry no stream frame");
}

#[tokio::test(start_paused = true)]
async fn a_working_turn_outlives_the_idle_ceiling() {
    let (tx, mut rx) = mpsc::channel(8);
    let (mut sink, log) = recording_sink();

    // Ten iterations 6s apart: 60s of work under a 10s idle ceiling. The flat
    // wall-clock cap this replaced would have killed it at 10s.
    let call = async move {
        for iteration in 1..=10u32 {
            tokio::time::sleep(Duration::from_secs(6)).await;
            tx.send(AgentProgress::IterationStarted {
                iteration,
                max_iterations: 10,
            })
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
        Err("openhuman task idle for 10000ms (no events)".to_string())
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
            tx.send(AgentProgress::TurnStarted)
                .await
                .expect("watchdog dropped the receiver");
        }
        tokio::time::sleep(Duration::from_secs(600)).await;
        "never observed"
    };

    let outcome = drive(call, &mut rx, &Abort::new(), 10_000, &mut sink).await;

    assert_eq!(
        outcome,
        Err("openhuman task idle for 10000ms (no events)".to_string())
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

    assert_eq!(outcome, Err("openhuman task aborted".to_string()));
}

#[tokio::test(start_paused = true)]
async fn events_queued_when_the_call_resolves_are_drained() {
    let (tx, mut rx) = mpsc::channel(8);
    let (mut sink, log) = recording_sink();

    // Nothing awaits between the sends and the return, so the call future
    // completes with both deltas and their boundary still sitting in the
    // channel.
    let call = async move {
        tx.send(AgentProgress::TextDelta {
            delta: "the ".to_string(),
            iteration: 1,
        })
        .await
        .expect("watchdog dropped the receiver");
        tx.send(AgentProgress::TextDelta {
            delta: "answer".to_string(),
            iteration: 1,
        })
        .await
        .expect("watchdog dropped the receiver");
        tx.send(AgentProgress::IterationStarted {
            iteration: 1,
            max_iterations: 1,
        })
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
    assert_eq!(log[1].0, "status");
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
        Err("openhuman task idle for 10000ms (no events)".to_string())
    );
}
