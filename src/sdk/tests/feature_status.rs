//! Coverage for the derived session-status machine branches that the inline
//! `tinyplace::status` tests do not reach: agent thinking/message derivations,
//! the lifecycle phase ladder, and the empty-call-id path.

use medulla::tinyplace::{
    initial_status, reduce_status, HarnessEventKind, LifecyclePayload, SemanticEvent, TextPayload,
    ToolCallPayload, STATE_IDLE, STATE_RUNNING, STATE_STOPPED,
};

fn sem(ms: i64, event: HarnessEventKind) -> SemanticEvent {
    SemanticEvent {
        timestamp_ms: Some(ms),
        event,
    }
}

#[test]
fn agent_thinking_and_message_derive_running() {
    let prev = initial_status(0);
    let thinking = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::AgentThinking(TextPayload {
                text: "pondering".into(),
            }),
        ),
    );
    assert_eq!(thinking.next.state, STATE_RUNNING);
    assert_eq!(thinking.next.detail, "thinking");

    let message = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::AgentMessage(TextPayload {
                text: "here you go".into(),
            }),
        ),
    );
    assert_eq!(message.next.state, STATE_RUNNING);
    assert_eq!(message.next.detail, "replying");
}

#[test]
fn lifecycle_phase_ladder() {
    let prev = initial_status(0);
    let phase = |p: &str| {
        reduce_status(
            &prev,
            &sem(
                1,
                HarnessEventKind::Lifecycle(LifecyclePayload { phase: p.into() }),
            ),
        )
    };

    let start = phase("session_start");
    assert_eq!(start.next.state, STATE_RUNNING);
    assert_eq!(start.next.detail, "working");

    let turn_start = phase("turn_start");
    assert_eq!(turn_start.next.detail, "working");

    let compact = phase("compact");
    assert_eq!(compact.next.state, STATE_RUNNING);
    assert_eq!(compact.next.detail, "compacting");

    let end = phase("session_end");
    assert_eq!(end.next.state, STATE_STOPPED);
}

#[test]
fn turn_end_ages_a_running_session_to_idle() {
    let prev = initial_status(0);
    // Move to running first, then a turn_end lifecycle returns to idle.
    let running = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::Lifecycle(LifecyclePayload {
                phase: "turn_start".into(),
            }),
        ),
    )
    .next;
    let end = reduce_status(
        &running,
        &sem(
            2,
            HarnessEventKind::Lifecycle(LifecyclePayload {
                phase: "turn_end".into(),
            }),
        ),
    );
    assert_eq!(end.next.state, STATE_IDLE);
    assert_eq!(end.next.detail, "idle");
}

#[test]
fn unknown_lifecycle_phase_carries_no_status_signal() {
    let prev = initial_status(0);
    let running = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::Lifecycle(LifecyclePayload {
                phase: "turn_start".into(),
            }),
        ),
    )
    .next;
    // An unrecognized phase keeps the prior state but advances the clock.
    let noop = reduce_status(
        &running,
        &sem(
            9,
            HarnessEventKind::Lifecycle(LifecyclePayload {
                phase: "mystery".into(),
            }),
        ),
    );
    assert!(noop.emit.is_none());
    assert_eq!(noop.next.state, STATE_RUNNING);
    assert_eq!(noop.next.last_event_at_ms, 9);
}

#[test]
fn tool_call_with_empty_call_id_has_no_active_call() {
    let prev = initial_status(0);
    let step = reduce_status(
        &prev,
        &sem(
            1,
            HarnessEventKind::ToolCall(ToolCallPayload {
                call_id: String::new(),
                tool_name: "Bash".into(),
                tool_kind: "shell".into(),
                display: "ls".into(),
                input: serde_json::Value::Null,
            }),
        ),
    );
    assert!(step.next.active_call_id.is_none());
}
