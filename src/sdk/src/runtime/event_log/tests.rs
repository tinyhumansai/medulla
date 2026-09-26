//! Tests for the shared runtime event-log retention and projection policy.

use crate::ui::events::{EventEnvelope, TuiEvent};

use super::{ThreadEventLog, CHAT_CAP, EVENT_CAP};

/// Build a deterministic event envelope for retention tests.
fn envelope(seq: u64, event: TuiEvent) -> EventEnvelope {
    EventEnvelope {
        seq,
        at: seq as i64,
        event,
    }
}

#[test]
fn projects_only_chat_visible_events() {
    let mut log = ThreadEventLog::default();
    log.push(envelope(
        1,
        TuiEvent::CycleStart {
            cycle_id: "cycle".into(),
        },
    ));
    log.push(envelope(
        2,
        TuiEvent::Assistant {
            body: "ready".into(),
        },
    ));

    assert_eq!(log.events.len(), 2);
    assert_eq!(log.chat_events.len(), 1);
    assert_eq!(log.chat_events[0].event.kind(), "assistant");
}

#[test]
fn trims_each_projection_to_its_own_cap() {
    let mut log = ThreadEventLog::default();
    for seq in 1..=(EVENT_CAP + CHAT_CAP + 3) as u64 {
        log.push(envelope(
            seq,
            TuiEvent::User {
                body: seq.to_string(),
            },
        ));
    }

    assert_eq!(log.events.len(), EVENT_CAP);
    assert_eq!(log.chat_events.len(), CHAT_CAP);
    assert!(log.events[0].seq < log.chat_events[0].seq);
}
