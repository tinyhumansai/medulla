//! Unit tests for the core runtime's wire normalization: event mapping (§3.3),
//! snapshot synthesis (§3.4), lane-key composition, and worker payload parsing.

use serde_json::{json, Value};

use crate::ui::agents::{derive_agent_lanes, TaskStatus};
use crate::ui::events::{EventEnvelope, TuiEvent};

use super::events::{compose_task_id, map_core_event, synth_from_snapshot};
use super::workers::workers_from_payload;

fn ev(cycle: &str, body: Value) -> TuiEvent {
    map_core_event(&body, cycle)
}

#[test]
fn task_complete_flat_wire_maps_to_nested_digest() {
    // §3.3(1): the wire body is flat; the mapper rebuilds the nested TaskDigest.
    let e = ev(
        "cyc:app:th_x:1",
        json!({"kind":"task_complete","taskId":"t1","status":"done","digest":"ok","usage":{"inputTokens":9,"outputTokens":2},"depth":1}),
    );
    match e {
        TuiEvent::TaskComplete { digest } => {
            assert_eq!(digest.task_id, "cyc:app:th_x:1/t:t1");
            assert_eq!(digest.status, "done");
            assert_eq!(digest.digest, "ok");
            assert_eq!(digest.usage.unwrap().input_tokens, 9);
        }
        other => panic!("expected task_complete, got {other:?}"),
    }
}

#[test]
fn cycle_id_folds_into_the_lane_key() {
    // §3.3(2): two cycles delegating the bare `t1` never collide into one lane.
    let events: Vec<EventEnvelope> = [
        ev(
            "cyc:app:th:1",
            json!({"kind":"task_start","taskId":"t1","instruction":"a","depth":1}),
        ),
        ev(
            "cyc:app:th:2",
            json!({"kind":"task_start","taskId":"t1","instruction":"b","depth":1}),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, event)| EventEnvelope {
        seq: i as u64,
        at: i as i64,
        event,
    })
    .collect();
    let lanes = derive_agent_lanes(&events, "CORE", &[]);
    let workers: Vec<_> = lanes
        .iter()
        .filter(|l| l.key.starts_with("worker:"))
        .collect();
    assert_eq!(workers.len(), 2, "two distinct cycle-scoped lanes expected");
}

#[test]
fn cancelled_status_is_distinct_from_failed() {
    // §3.3(3): cancelled ≠ failed.
    let events = vec![EventEnvelope {
        seq: 1,
        at: 1,
        event: ev(
            "cyc:app:th:1",
            json!({"kind":"task_complete","taskId":"t1","status":"cancelled","digest":""}),
        ),
    }];
    let lanes = derive_agent_lanes(&events, "CORE", &[]);
    let lane = lanes.iter().find(|l| l.key.starts_with("worker:")).unwrap();
    assert_eq!(lane.tasks[0].status, TaskStatus::Cancelled);
}

#[test]
fn task_complete_without_task_start_still_lands_a_lane() {
    // §3.3(4): a completion whose task_start was evicted is not dropped.
    let events = vec![EventEnvelope {
        seq: 1,
        at: 1,
        event: ev(
            "cyc:app:th:1",
            json!({"kind":"task_complete","taskId":"orphan","status":"done","digest":"d"}),
        ),
    }];
    let lanes = derive_agent_lanes(&events, "CORE", &[]);
    let lane = lanes.iter().find(|l| l.key.starts_with("worker:"));
    assert!(lane.is_some(), "orphan completion must still create a lane");
    assert_eq!(lane.unwrap().tasks[0].status, TaskStatus::Done);
}

#[test]
fn snapshot_rebuild_synthesizes_events() {
    let snapshot = json!({
        "at": 1000,
        "chat": [{"seq":1,"role":"user","body":"hi"},{"seq":2,"role":"assistant","body":"yo"}],
        "tasks": [{"taskId":"t1","cycleId":"cyc:app:th:1","status":"done","instruction":"go","digest":"done"}],
    });
    let mut seq = 0;
    let synth = synth_from_snapshot(&snapshot, &mut seq);
    let kinds: Vec<&str> = synth.iter().map(|e| e.event.kind()).collect();
    assert_eq!(
        kinds,
        vec!["user", "assistant", "task_start", "task_complete"]
    );
    assert_eq!(seq, 4);
}

#[test]
fn compose_task_id_without_cycle_returns_bare_id() {
    // §3.3(2): an empty cycle id leaves the bare task id unqualified.
    assert_eq!(compose_task_id("", "t1"), "t1");
    assert_eq!(compose_task_id("cyc", "t1"), "cyc/t:t1");
}

#[test]
fn task_complete_empty_status_defaults_to_done() {
    // §3.3(1): a completion with no status on the wire folds to "done".
    let e = ev(
        "cyc:app:th:1",
        json!({"kind":"task_complete","taskId":"t1","digest":"ok"}),
    );
    match e {
        TuiEvent::TaskComplete { digest } => assert_eq!(digest.status, "done"),
        other => panic!("expected task_complete, got {other:?}"),
    }
}

#[test]
fn unknown_kind_rides_through_as_unknown() {
    // An unrecognized kind is preserved rather than dropped.
    let e = ev("cyc", json!({"kind":"totally_novel","foo":"bar"}));
    match e {
        TuiEvent::Unknown { kind, data } => {
            assert_eq!(kind, "totally_novel");
            assert_eq!(data.get("foo").and_then(Value::as_str), Some("bar"));
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn synth_skips_non_chat_roles_and_folds_last_event() {
    // A `system` chat row is skipped (§3.4); a running task with a `lastEvent`
    // synthesizes task_start + task_event but no task_complete.
    let snapshot = json!({
        "at": 5,
        "chat": [
            {"role":"system","body":"noise"},
            {"role":"user","body":"hi"}
        ],
        "tasks": [{
            "taskId":"t1","cycleId":"cyc:app:th:1","status":"running",
            "instruction":"go","depth":2,"agentId":"dev-1","harness":"codex",
            "lastEvent": {"eventKind":"text","content":"reading…"}
        }],
    });
    let mut seq = 0;
    let synth = synth_from_snapshot(&snapshot, &mut seq);
    let kinds: Vec<&str> = synth.iter().map(|e| e.event.kind()).collect();
    assert_eq!(kinds, vec!["user", "task_start", "task_event"]);
}

#[test]
fn workers_payload_parses_rows() {
    let payload = json!({
        "workers": [
            {"id":"w_1","address":"@dev","handle":"@dev","harness":"claude","selected":true},
            {"id":"w_2","address":"addr2"}
        ],
        "selectedId": "w_1"
    });
    let rows = workers_from_payload(&payload);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].selected);
    assert_eq!(rows[0].harness.as_deref(), Some("claude"));
    assert!(!rows[1].selected);
}
