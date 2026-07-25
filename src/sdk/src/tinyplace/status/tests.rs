//! Tests for the status module.

use crate::tinyplace::status::{
    initial_status, reduce_status, tick_status, SemanticEvent, DEFAULT_IDLE_AFTER_MS,
};
use crate::tinyplace::{
    ApprovalRequestPayload, ErrorPayload, HarnessEventKind, LifecyclePayload, StatusPayload,
    ToolCallPayload, ToolResultPayload, UnknownPayload, UserPromptPayload, STATE_ERRORED,
    STATE_IDLE, STATE_RUNNING, STATE_RUNNING_TOOL, STATE_STOPPED, STATE_WAITING_APPROVAL,
};

fn sem(ms: i64, event: HarnessEventKind) -> SemanticEvent {
    SemanticEvent {
        timestamp_ms: Some(ms),
        event,
    }
}

fn tool_call(name: &str, display: &str, call_id: &str) -> HarnessEventKind {
    HarnessEventKind::ToolCall(ToolCallPayload {
        call_id: call_id.to_string(),
        tool_name: name.to_string(),
        tool_kind: "shell".to_string(),
        display: display.to_string(),
        input: serde_json::Value::Null,
    })
}

fn tool_result_ok() -> HarnessEventKind {
    HarnessEventKind::ToolResult(ToolResultPayload {
        call_id: "c".to_string(),
        ok: true,
        exit_code: None,
        is_error: false,
        output: "o".to_string(),
        output_bytes: 1,
    })
}

#[test]
fn tool_call_moves_to_running_tool_and_emits() {
    let prev = initial_status(0);
    let step = reduce_status(&prev, &sem(1000, tool_call("Bash", "npm test", "c1")));
    assert_eq!(step.next.state, STATE_RUNNING_TOOL);
    assert_eq!(step.next.detail, "running Bash: npm test");
    assert_eq!(step.next.active_call_id.as_deref(), Some("c1"));
    let emit = step.emit.expect("state changed, must emit");
    assert_eq!(emit.state, STATE_RUNNING_TOOL);
    assert_eq!(emit.active_call_id.as_deref(), Some("c1"));
    assert_eq!(step.next.last_event_at_ms, 1000);
}

#[test]
fn identical_derived_state_is_change_gated() {
    let prev = initial_status(0);
    // A user_prompt derives running/"working"; the machine starts idle so this
    // first one emits.
    let step = reduce_status(
        &prev,
        &sem(
            100,
            HarnessEventKind::UserPrompt(UserPromptPayload {
                text: "hi".to_string(),
                source: "human".to_string(),
            }),
        ),
    );
    assert!(step.emit.is_some());
    // tool_result -> running/"processing" differs from "working" -> emits.
    let again = reduce_status(&step.next, &sem(200, tool_result_ok()));
    assert!(again.emit.is_some());
    let repeat = reduce_status(&again.next, &sem(300, tool_result_ok()));
    // Same running/"processing" as before — no change, no emit.
    assert!(repeat.emit.is_none());
    assert_eq!(repeat.next.last_event_at_ms, 300);
}

#[test]
fn signalless_event_keeps_state_but_advances_clock() {
    let prev = initial_status(0);
    let moved = reduce_status(&prev, &sem(500, tool_call("Bash", "x", "c1")));
    // An Unknown event carries no status signal.
    let unknown = reduce_status(
        &moved.next,
        &sem(
            900,
            HarnessEventKind::Unknown(UnknownPayload {
                raw: serde_json::Value::Null,
            }),
        ),
    );
    assert!(unknown.emit.is_none());
    assert_eq!(unknown.next.state, STATE_RUNNING_TOOL);
    assert_eq!(unknown.next.last_event_at_ms, 900);
}

#[test]
fn zero_or_missing_timestamp_falls_back() {
    let prev = initial_status(42);
    let step = reduce_status(
        &prev,
        &SemanticEvent {
            timestamp_ms: None,
            event: tool_call("Bash", "x", "c1"),
        },
    );
    assert_eq!(step.next.last_event_at_ms, 42);
    let zero = reduce_status(
        &prev,
        &SemanticEvent {
            timestamp_ms: Some(0),
            event: tool_call("Bash", "x", "c1"),
        },
    );
    assert_eq!(zero.next.last_event_at_ms, 42);
}

#[test]
fn approval_and_error_and_lifecycle_derivations() {
    let prev = initial_status(0);
    let approval = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::ApprovalRequest(ApprovalRequestPayload {
                call_id: Some("c9".to_string()),
                tool_name: "Bash".to_string(),
                display: "rm".to_string(),
                reason: None,
            }),
        ),
    );
    assert_eq!(approval.next.state, STATE_WAITING_APPROVAL);
    assert_eq!(approval.next.detail, "awaiting approval: rm");
    assert_eq!(approval.next.active_call_id.as_deref(), Some("c9"));

    let fatal = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::Error(ErrorPayload {
                message: "boom".to_string(),
                fatal: true,
            }),
        ),
    );
    assert_eq!(fatal.next.state, STATE_ERRORED);

    let nonfatal = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::Error(ErrorPayload {
                message: "warn".to_string(),
                fatal: false,
            }),
        ),
    );
    assert_eq!(nonfatal.next.state, STATE_RUNNING);

    let end = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::Lifecycle(LifecyclePayload {
                phase: "session_end".to_string(),
            }),
        ),
    );
    assert_eq!(end.next.state, STATE_STOPPED);
}

#[test]
fn status_event_passes_through_verbatim() {
    let prev = initial_status(0);
    let step = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::Status(StatusPayload {
                state: STATE_WAITING_APPROVAL.to_string(),
                detail: "custom".to_string(),
                active_call_id: Some("cc".to_string()),
            }),
        ),
    );
    assert_eq!(step.next.state, STATE_WAITING_APPROVAL);
    assert_eq!(step.next.detail, "custom");
    assert_eq!(step.next.active_call_id.as_deref(), Some("cc"));
}

#[test]
fn detail_is_capped_to_one_line() {
    let prev = initial_status(0);
    let long = "a".repeat(300);
    let step = reduce_status(&prev, &sem(1, tool_call("Bash", &long, "c1")));
    let chars = step.next.detail.chars().count();
    assert!(chars <= 120, "detail should be capped, got {chars}");
    assert!(step.next.detail.ends_with('…'));

    let multi = reduce_status(&prev, &sem(1, tool_call("Bash", "line1\nline2", "c1")));
    assert_eq!(multi.next.detail, "running Bash: line1");
}

#[test]
fn tick_ages_active_session_to_idle() {
    let prev = initial_status(0);
    let running = reduce_status(&prev, &sem(1000, tool_call("Bash", "x", "c1"))).next;
    // Not yet stale.
    let fresh = tick_status(&running, 1000 + 10_000, DEFAULT_IDLE_AFTER_MS, false);
    assert!(fresh.emit.is_none());
    assert_eq!(fresh.next.state, STATE_RUNNING_TOOL);
    // Past the idle horizon.
    let stale = tick_status(
        &running,
        1000 + DEFAULT_IDLE_AFTER_MS,
        DEFAULT_IDLE_AFTER_MS,
        false,
    );
    let emit = stale.emit.expect("aging to idle emits");
    assert_eq!(emit.state, STATE_IDLE);
    assert_eq!(stale.next.state, STATE_IDLE);
}

#[test]
fn tick_heartbeat_reemits_without_state_change() {
    let prev = initial_status(0);
    let running = reduce_status(&prev, &sem(1000, tool_call("Bash", "x", "c1"))).next;
    let beat = tick_status(&running, 1000 + 5_000, DEFAULT_IDLE_AFTER_MS, true);
    let emit = beat.emit.expect("heartbeat re-emits");
    assert_eq!(emit.state, STATE_RUNNING_TOOL);
    assert_eq!(beat.next, running);
}

#[test]
fn tick_does_not_idle_an_already_idle_session() {
    let prev = initial_status(0);
    let quiet = tick_status(&prev, 1_000_000, DEFAULT_IDLE_AFTER_MS, false);
    assert!(quiet.emit.is_none());
    assert_eq!(quiet.next.state, STATE_IDLE);
}
