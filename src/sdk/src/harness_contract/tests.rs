//! Round-trip serde tests against JSON literals written by hand from the
//! medulla-v1 TypeScript definitions. Each test asserts the exact camelCase /
//! lowercase field and tag names the TS wire shape emits, so a rename that
//! diverges from the source of truth fails here.

use super::*;
use serde_json::{json, Value};

/// Deserialize `literal`, re-serialize, and assert the JSON is value-equal to the
/// original — proving both directions agree on every field name.
fn round_trip<T>(literal: Value) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let parsed: T = serde_json::from_value(literal.clone()).expect("deserialize");
    let reserialized = serde_json::to_value(&parsed).expect("serialize");
    assert_eq!(reserialized, literal, "round-trip changed the JSON");
    parsed
}

#[test]
fn tracked_task_round_trips_with_camel_case() {
    let literal = json!({
        "id": "task-1",
        "title": "Wire the harness contract",
        "detail": "Mirror the TS shapes",
        "status": "active",
        "createdAt": "2026-07-20T00:00:00.000Z",
        "updatedAt": "2026-07-20T00:05:00.000Z",
        "instructionId": "inst-agent-0",
        "delegatedTaskIds": ["deleg-1", "deleg-2"],
        "notes": ["kicked off", "half done"]
    });
    let task: TrackedTask = round_trip(literal);
    assert_eq!(task.status, TrackedTaskStatus::Active);
    assert_eq!(task.delegated_task_ids.len(), 2);
}

#[test]
fn tracked_task_omits_optionals_when_absent() {
    // The minimal shape: no detail, no instructionId, empty arrays present.
    let literal = json!({
        "id": "task-2",
        "title": "Minimal",
        "status": "open",
        "createdAt": "2026-07-20T00:00:00.000Z",
        "updatedAt": "2026-07-20T00:00:00.000Z",
        "delegatedTaskIds": [],
        "notes": []
    });
    let task: TrackedTask = round_trip(literal);
    assert!(task.detail.is_none());
    assert!(task.instruction_id.is_none());
    // The optionals must not leak into the serialized JSON.
    let out = serde_json::to_value(&task).unwrap();
    assert!(out.get("detail").is_none());
    assert!(out.get("instructionId").is_none());
}

#[test]
fn every_tracked_task_status_serializes_lowercase() {
    let cases = [
        (TrackedTaskStatus::Open, "open"),
        (TrackedTaskStatus::Active, "active"),
        (TrackedTaskStatus::Blocked, "blocked"),
        (TrackedTaskStatus::Done, "done"),
        (TrackedTaskStatus::Cancelled, "cancelled"),
    ];
    for (status, wire) in cases {
        assert_eq!(serde_json::to_value(status).unwrap(), json!(wire));
        let back: TrackedTaskStatus = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(back, status);
    }
}

#[test]
fn harness_status_round_trips() {
    let literal = json!({
        "state": "running",
        "queued": 2,
        "activeInstructionId": "inst-agent-3",
        "activeCycleId": "cycle-agent-3",
        "tasks": [{
            "id": "task-1",
            "title": "Do the thing",
            "status": "active",
            "createdAt": "2026-07-20T00:00:00.000Z",
            "updatedAt": "2026-07-20T00:01:00.000Z",
            "delegatedTaskIds": [],
            "notes": []
        }],
        "runningDelegations": 1,
        "usage": { "cycles": 4, "inputTokens": 12000, "outputTokens": 3400 },
        "lastResult": { "reply": "done", "escalations": [] },
        "escalations": ["needs human review"]
    });
    let status: HarnessStatus = round_trip(literal);
    assert_eq!(status.state, HarnessState::Running);
    assert_eq!(status.usage.input_tokens, 12000);
    assert_eq!(status.tasks.len(), 1);
    // last_result is preserved opaquely.
    assert_eq!(
        status.last_result.as_ref().unwrap().get("reply").unwrap(),
        "done"
    );
}

#[test]
fn harness_status_idle_omits_active_ids() {
    let literal = json!({
        "state": "idle",
        "queued": 0,
        "tasks": [],
        "runningDelegations": 0,
        "usage": { "cycles": 0, "inputTokens": 0, "outputTokens": 0 },
        "escalations": []
    });
    let status: HarnessStatus = round_trip(literal);
    assert_eq!(status.state, HarnessState::Idle);
    assert!(status.active_instruction_id.is_none());
    assert!(status.last_result.is_none());
}

#[test]
fn harness_event_lifecycle_kinds_are_distinct_tags() {
    for kind in ["instruction_queued", "cycle_start", "cycle_end"] {
        let literal = json!({
            "kind": kind,
            "instructionId": "inst-agent-0",
            "cycleId": "cycle-agent-0"
        });
        let event: HarnessEvent = round_trip(literal);
        match (kind, &event) {
            ("instruction_queued", HarnessEvent::InstructionQueued { .. })
            | ("cycle_start", HarnessEvent::CycleStart { .. })
            | ("cycle_end", HarnessEvent::CycleEnd { .. }) => {}
            _ => panic!("kind {kind} mapped to the wrong variant: {event:?}"),
        }
    }
}

#[test]
fn harness_event_task_board_changed_round_trips() {
    let literal = json!({
        "kind": "task_board_changed",
        "task": {
            "id": "task-9",
            "title": "Changed",
            "status": "done",
            "createdAt": "2026-07-20T00:00:00.000Z",
            "updatedAt": "2026-07-20T00:09:00.000Z",
            "delegatedTaskIds": [],
            "notes": []
        }
    });
    let event: HarnessEvent = round_trip(literal);
    match event {
        HarnessEvent::TaskBoardChanged { task } => {
            assert_eq!(task.status, TrackedTaskStatus::Done);
        }
        other => panic!("expected task_board_changed, got {other:?}"),
    }
}

#[test]
fn harness_event_cycle_event_preserves_opaque_payload() {
    let literal = json!({
        "kind": "cycle_event",
        "event": { "kind": "inference_end", "usage": { "inputTokens": 10, "outputTokens": 2 } }
    });
    let event: HarnessEvent = round_trip(literal);
    match event {
        HarnessEvent::CycleEvent { event } => {
            assert_eq!(event.get("kind").unwrap(), "inference_end");
        }
        other => panic!("expected cycle_event, got {other:?}"),
    }
}

#[test]
fn instruction_receipt_round_trips() {
    let literal = json!({ "instructionId": "inst-agent-0", "cycleId": "cycle-agent-0" });
    let receipt: InstructionReceipt = round_trip(literal);
    assert_eq!(receipt.instruction_id, "inst-agent-0");
}

#[test]
fn agent_budget_metadata_round_trips_with_iso_reset() {
    // The roster-facing stamp: primaryResetsAt is an ISO string here.
    let literal = json!({
        "seatId": "seat-anthropic-1",
        "provider": "anthropic",
        "plan": "claude_max_5x",
        "planLabel": "Claude Max 5×",
        "headroomTokens": 1_250_000,
        "exhausted": false,
        "primaryResetsAt": "2026-07-20T05:00:00.000Z"
    });
    let budget: AgentBudgetMetadata = round_trip(literal);
    assert_eq!(budget.plan_label, "Claude Max 5×");
    assert_eq!(budget.headroom_tokens, 1_250_000);
}

#[test]
fn seat_headroom_round_trips_with_epoch_ms() {
    // SeatHeadroom carries epoch-ms numbers throughout (not ISO strings).
    let literal = json!({
        "seatId": "seat-anthropic-1",
        "provider": "anthropic",
        "plan": "claude_max_5x",
        "planLabel": "Claude Max 5×",
        "agentIds": ["agent-a"],
        "enabled": true,
        "priority": 0,
        "headroomTokens": 1_250_000,
        "exhausted": false,
        "throttledUntil": 1_795_000_000_000i64,
        "primaryResetsAt": 1_795_000_000_000i64,
        "perWindow": {
            "primary": { "remaining": 1_250_000, "resetsAt": 1_795_000_000_000i64 },
            "secondary": { "remaining": 17_500_000, "resetsAt": 1_795_600_000_000i64 }
        }
    });
    let seat: SeatHeadroom = round_trip(literal);
    assert_eq!(seat.priority, 0);
    assert_eq!(seat.per_window.len(), 2);
    assert_eq!(seat.per_window["primary"].remaining, 1_250_000);
}

#[test]
fn seat_headroom_omits_throttled_until_when_absent() {
    let literal = json!({
        "seatId": "seat-openai-1",
        "provider": "openai",
        "plan": "chatgpt_pro",
        "planLabel": "ChatGPT Pro",
        "agentIds": [],
        "enabled": true,
        "priority": 1,
        "headroomTokens": 0,
        "exhausted": true,
        "primaryResetsAt": 1_795_000_000_000i64,
        "perWindow": {}
    });
    let seat: SeatHeadroom = round_trip(literal);
    assert!(seat.throttled_until.is_none());
    assert!(seat.exhausted);
}

#[test]
fn budget_metadata_parses_out_of_descriptor_metadata() {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "budget".into(),
        json!({
            "seatId": "seat-1",
            "provider": "anthropic",
            "plan": "claude_pro",
            "planLabel": "Claude Pro",
            "headroomTokens": 42_000,
            "exhausted": false,
            "primaryResetsAt": "2026-07-20T05:00:00.000Z"
        }),
    );
    let parsed = AgentBudgetMetadata::from_metadata(&metadata).expect("budget present");
    assert_eq!(parsed.seat_id, "seat-1");

    // Absent or malformed → None, never an error.
    assert!(AgentBudgetMetadata::from_metadata(&serde_json::Map::new()).is_none());
    let mut bad = serde_json::Map::new();
    bad.insert("budget".into(), json!({ "seatId": 5 }));
    assert!(AgentBudgetMetadata::from_metadata(&bad).is_none());
}

#[test]
fn reserved_tool_names_match_the_ts_modules() {
    for name in [
        "task_create",
        "task_update",
        "task_list",
        "memory_write",
        "memory_read",
        "memory_list",
    ] {
        assert!(is_reserved_tool_name(name), "{name} should be reserved");
    }
    assert!(!is_reserved_tool_name("web_search"));
}
