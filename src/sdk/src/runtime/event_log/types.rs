//! The bounded per-thread event-log type and its retention policy.

use crate::ui::events::{EventEnvelope, TuiEvent};

/// Upper bound on the raw event history retained for one thread.
pub(crate) const EVENT_CAP: usize = 5000;
/// Upper bound on the chat-visible event history retained for one thread.
pub(crate) const CHAT_CAP: usize = 2000;

/// A runtime thread's full event history and chat-visible projection.
#[derive(Clone, Debug, Default)]
pub(crate) struct ThreadEventLog {
    /// Every retained event, in sequence order.
    pub(crate) events: Vec<EventEnvelope>,
    /// Retained user, assistant, and error events, in sequence order.
    pub(crate) chat_events: Vec<EventEnvelope>,
}

impl ThreadEventLog {
    /// Append an envelope, project chat-visible events, and enforce both caps.
    pub(crate) fn push(&mut self, envelope: EventEnvelope) {
        let chat_visible = matches!(
            envelope.event,
            TuiEvent::User { .. } | TuiEvent::Assistant { .. } | TuiEvent::Error { .. }
        );
        self.events.push(envelope.clone());
        trim_to_cap(&mut self.events, EVENT_CAP);
        if chat_visible {
            self.chat_events.push(envelope);
            trim_to_cap(&mut self.chat_events, CHAT_CAP);
        }
    }

    /// Remove both projections while retaining their allocated capacity.
    pub(crate) fn clear(&mut self) {
        self.events.clear();
        self.chat_events.clear();
    }
}

/// Drop the oldest entries when a bounded log exceeds its retention limit.
fn trim_to_cap(events: &mut Vec<EventEnvelope>, cap: usize) {
    if events.len() > cap {
        events.drain(0..events.len() - cap);
    }
}
