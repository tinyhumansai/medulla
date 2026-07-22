//! Event-fold and state-model tests: [`fold_event`] driving the running flag,
//! task board, and transcript; [`CoreState::reset_for_replay`],
//! [`CoreState::stream_health`], and [`CoreState::describe`]; and the
//! event/chat-log cap and chattiness behaviour.

use serde_json::json;

use crate::harness_contract::{HarnessState, TrackedTaskStatus};
use crate::runtime::StreamState;
use crate::ui::events::TuiEvent;

use super::super::protocol::fold_event;
use super::super::types::{ConnState, CoreError, CoreState};

#[test]
fn fold_event_drives_running_and_board() {
    let mut s = CoreState::new();

    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_start","instructionId":"i","cycleId":"c"})
    ));
    assert!(s.running);
    assert_eq!(s.harness.as_ref().unwrap().state, HarnessState::Running);

    let task = json!({"kind":"task_board_changed","task":{
        "id":"t1","title":"do","status":"open","createdAt":"0","updatedAt":"0",
        "delegatedTaskIds":[],"notes":[]}});
    assert!(fold_event(&mut s, &task));
    assert_eq!(s.harness.as_ref().unwrap().tasks.len(), 1);
    // A second board change with the same id updates in place.
    let task2 = json!({"kind":"task_board_changed","task":{
        "id":"t1","title":"do","status":"done","createdAt":"0","updatedAt":"1",
        "delegatedTaskIds":[],"notes":[]}});
    assert!(fold_event(&mut s, &task2));
    let tasks = &s.harness.as_ref().unwrap().tasks;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, TrackedTaskStatus::Done);

    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_end","instructionId":"i","cycleId":"c"})
    ));
    assert!(!s.running);
    assert_eq!(s.harness.as_ref().unwrap().state, HarnessState::Idle);
    assert_eq!(s.harness.as_ref().unwrap().usage.cycles, 1);
    assert!(s.last_result.is_some());
}

#[test]
fn fold_event_streams_task_board_changes_into_the_event_log() {
    // Codex review finding: task_board_changed used to update
    // `RuntimeSnapshot.harness` only, so headless `run` consumers (which drain
    // `snapshot().events`) silently lost task progress. The fold must also
    // append an event envelope carrying the changed row.
    let mut s = CoreState::new();
    assert!(fold_event(
        &mut s,
        &json!({"kind":"task_board_changed","task":{
            "id":"t1","title":"reconcile","status":"active","createdAt":"0","updatedAt":"0",
            "delegatedTaskIds":[],"notes":[]}})
    ));

    // The board updated, and the same change landed in the event log.
    assert_eq!(s.harness.as_ref().unwrap().tasks.len(), 1);
    assert_eq!(s.events.len(), 1);
    let env = &s.events[0];
    assert_eq!(env.event.kind(), "task_board_changed");
    let json = serde_json::to_value(&env.event).unwrap();
    assert_eq!(json["kind"], "task_board_changed");
    assert_eq!(json["task"]["id"], "t1");
    assert_eq!(json["task"]["status"], "active");

    // An update to the same task streams a second envelope (one per change).
    assert!(fold_event(
        &mut s,
        &json!({"kind":"task_board_changed","task":{
            "id":"t1","title":"reconcile","status":"done","createdAt":"0","updatedAt":"1",
            "delegatedTaskIds":[],"notes":[]}})
    ));
    assert_eq!(s.harness.as_ref().unwrap().tasks.len(), 1);
    assert_eq!(s.events.len(), 2);
    let json = serde_json::to_value(&s.events[1].event).unwrap();
    assert_eq!(json["task"]["status"], "done");
}

#[test]
fn fold_event_passes_unknown_and_cycle_events_through() {
    let mut s = CoreState::new();
    // A serve-level roster_event is not a HarnessEvent: kept verbatim.
    assert!(fold_event(
        &mut s,
        &json!({"kind":"roster_event","agent":{"id":"a"}})
    ));
    // instruction_queued increments the queue counter.
    assert!(fold_event(
        &mut s,
        &json!({"kind":"instruction_queued","instructionId":"i","cycleId":"c"})
    ));
    assert_eq!(s.harness.as_ref().unwrap().queued, 1);
    // A cycle_event wraps an inner cycle event, which rides through as an event row.
    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_event","event":{"kind":"inference_end"}})
    ));
    assert!(!s.events.is_empty());
}

#[test]
fn fold_event_drains_queue_when_a_cycle_starts() {
    let mut s = CoreState::new();

    // Two instructions queue up behind the running one: backlog climbs to 2.
    for _ in 0..2 {
        assert!(fold_event(
            &mut s,
            &json!({"kind":"instruction_queued","instructionId":"i","cycleId":"c"})
        ));
    }
    assert_eq!(s.harness.as_ref().unwrap().queued, 2);

    // Each cycle_start dequeues one instruction, draining the backlog back to 0.
    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_start","instructionId":"i1","cycleId":"c1"})
    ));
    assert_eq!(s.harness.as_ref().unwrap().queued, 1);
    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_start","instructionId":"i2","cycleId":"c2"})
    ));
    assert_eq!(s.harness.as_ref().unwrap().queued, 0);

    // A cycle_start with no queued instruction behind it never underflows.
    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_start","instructionId":"i3","cycleId":"c3"})
    ));
    assert_eq!(s.harness.as_ref().unwrap().queued, 0);
}

#[test]
fn fold_event_appends_assistant_turns_into_messages() {
    // Codex review finding #2: an assistant cycle_event must reach the
    // rendered transcript (`CoreState::messages`), not just the event log —
    // that's what `CoreRuntime::snapshot` builds `messages`/turn counts from.
    let mut s = CoreState::new();
    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_event","event":{"kind":"assistant","body":"hi there"}})
    ));
    assert_eq!(s.messages.len(), 1);
    assert_eq!(s.messages[0].role, "assistant");
    assert_eq!(s.messages[0].content, "hi there");
    // It is also chat-visible in the event log.
    assert!(s
        .chat_events
        .iter()
        .any(|e| matches!(&e.event, TuiEvent::Assistant { body } if body == "hi there")));
}

#[test]
fn fold_event_deduplicates_the_optimistic_user_echo() {
    // `submit` (runtime_impl.rs) optimistically pushes the user's turn and
    // records it in `pending_user_echo`; when the wire reflects that same
    // turn back over `cycle_event`, it must not be appended a second time.
    let mut s = CoreState::new();
    s.messages.push(crate::ui::chat_store::ChatMessage {
        role: "user".into(),
        content: "hello".into(),
    });
    s.pending_user_echo = Some("hello".into());

    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_event","event":{"kind":"user","body":"hello"}})
    ));
    assert_eq!(s.messages.len(), 1, "the echo must not double the turn");
    assert!(s.pending_user_echo.is_none(), "the echo clears the marker");

    // A distinct user turn (not the pending echo) is not swallowed.
    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_event","event":{"kind":"user","body":"world"}})
    ));
    assert_eq!(s.messages.len(), 2);
}

#[test]
fn reset_for_replay_clears_fold_derived_state_but_keeps_identity() {
    let mut s = CoreState::new();
    s.session_id = "agent".into();
    s.serve_version = Some("3.12.0".into());
    s.async_mode = true;
    // Fold a full cycle so counters, tasks, and the event log are all populated.
    fold_event(
        &mut s,
        &json!({"kind":"cycle_start","instructionId":"i","cycleId":"c"}),
    );
    fold_event(
        &mut s,
        &json!({"kind":"task_board_changed","task":{
            "id":"t1","title":"do","status":"active","createdAt":"0","updatedAt":"0",
            "delegatedTaskIds":[],"notes":[]}}),
    );
    fold_event(
        &mut s,
        &json!({"kind":"cycle_end","instructionId":"i","cycleId":"c"}),
    );
    assert!(s.harness.is_some());
    assert!(!s.events.is_empty());

    s.reset_for_replay();

    // Fold-derived state is gone so a replay rebuilds it from scratch.
    assert!(s.harness.is_none());
    assert!(s.events.is_empty());
    assert!(s.chat_events.is_empty());
    assert!(s.messages.is_empty());
    assert!(s.last_result.is_none());
    assert!(!s.running);
    assert_eq!(s.seq, 0);
    // Connection-spanning identity and local toggles survive.
    assert_eq!(s.session_id, "agent");
    assert_eq!(s.serve_version.as_deref(), Some("3.12.0"));
    assert!(s.async_mode);
}

#[test]
fn reset_for_replay_preserves_the_unacked_optimistic_user_turn() {
    // Codex review finding: a user turn submitted just before a connection drop
    // is only optimistic local state — the serve replay re-sends *events*, and
    // the turn may never have reached serve — so `reset_for_replay` must carry
    // it across the reset instead of wiping what the operator typed.
    let mut s = CoreState::new();
    // Mirror what `submit` does: optimistic message + echo marker + User event.
    s.messages.push(crate::ui::chat_store::ChatMessage {
        role: "user".into(),
        content: "ship it".into(),
    });
    s.pending_user_echo = Some("ship it".into());
    s.emit(TuiEvent::User {
        body: "ship it".into(),
    });

    s.reset_for_replay();

    // The un-acked turn survives, in both the transcript and the event log.
    assert_eq!(s.messages.len(), 1);
    assert_eq!(s.messages[0].role, "user");
    assert_eq!(s.messages[0].content, "ship it");
    assert!(s
        .events
        .iter()
        .any(|e| matches!(&e.event, TuiEvent::User { body } if body == "ship it")));
    // The echo marker stays armed, so a replayed echo still folds into exactly
    // one transcript row rather than doubling the preserved copy.
    assert_eq!(s.pending_user_echo.as_deref(), Some("ship it"));
    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_event","event":{"kind":"user","body":"ship it"}})
    ));
    assert_eq!(s.messages.len(), 1, "the replayed echo must not double up");
    assert!(s.pending_user_echo.is_none());
}

#[test]
fn stream_health_maps_conn_and_gap() {
    let mut s = CoreState::new();
    assert_eq!(s.stream_health(), StreamState::Resyncing); // Connecting
    s.conn = ConnState::Live;
    assert_eq!(s.stream_health(), StreamState::Live);

    // A non-contiguous protocol seq latches a gap → Resyncing.
    s.note_stream_seq(1);
    s.note_stream_seq(2);
    assert!(!s.gap);
    s.note_stream_seq(9);
    assert!(s.gap);
    assert_eq!(s.stream_health(), StreamState::Resyncing);

    // A fresh connection resets the cursor.
    s.reset_stream_cursor();
    assert!(!s.gap);
    assert_eq!(s.stream_health(), StreamState::Live);

    s.conn = ConnState::Reconnecting;
    assert_eq!(s.stream_health(), StreamState::Resyncing);
    s.conn = ConnState::Unavailable("boom".into());
    assert_eq!(s.stream_health(), StreamState::Stalled);
}

#[test]
fn describe_reflects_lifecycle_and_error_display() {
    let mut s = CoreState::new();
    assert!(s.describe().contains("connecting"));
    s.serve_version = Some("3.12.0".into());
    s.conn = ConnState::Live;
    assert!(s.describe().contains("3.12.0") && s.describe().contains("attached"));
    s.conn = ConnState::Reconnecting;
    assert!(s.describe().contains("reconnecting"));
    s.conn = ConnState::Unavailable("bad".into());
    assert!(s.describe().contains("unavailable: bad"));

    let e = CoreError::transport("dropped");
    assert!(e.to_string().contains("dropped") && e.to_string().contains("internal"));
}

#[test]
fn event_log_and_chat_log_respect_caps_and_chattiness() {
    let mut s = CoreState::new();
    // A chat-visible event lands in both logs; a non-chat event only in events.
    s.emit(TuiEvent::User { body: "hi".into() });
    s.emit(TuiEvent::CycleStart {
        cycle_id: "c".into(),
    });
    assert_eq!(s.events.len(), 2);
    assert_eq!(s.chat_events.len(), 1);
    assert!(s.events[0].seq < s.events[1].seq);
}
