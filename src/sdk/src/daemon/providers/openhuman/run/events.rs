//! The turn's outgoing event stream: one place that owns the caller's callback,
//! the transcript line counter, and the count of what was actually produced.
//!
//! Kept as a type rather than three loose `&mut` parameters because the three
//! move together — every emission advances the line, bumps the count, and (when
//! there is one) reaches the callback. A caller that could bump one without the
//! others is a caller that can report a count the transcript does not support.

use serde_json::Value;

use crate::protocol::HarnessEvent;

use super::super::super::types::OnEvent;
use super::core_contract::AgentProgress;
use super::progress::semantic_events;

/// Accumulating sink for the semantic events one turn produces.
pub(super) struct EventSink {
    /// The caller's per-event callback, when it registered one.
    on_event: Option<OnEvent>,
    /// Number of events emitted so far — what [`super::super::super::types::RunTaskResult::events`]
    /// reports.
    emitted: usize,
}

impl EventSink {
    /// A sink feeding `on_event`, or counting silently when it is `None`.
    pub(super) fn new(on_event: Option<OnEvent>) -> Self {
        Self {
            on_event,
            emitted: 0,
        }
    }

    /// Emit one event of `kind` carrying `payload`.
    ///
    /// The count advances whether or not a callback is registered: the number
    /// describes the turn, not the observer.
    pub(super) fn emit(&mut self, kind: &str, payload: Value) {
        let line = self.emitted as i64;
        self.emitted += 1;
        let Some(callback) = self.on_event.as_mut() else {
            return;
        };
        callback(&crate::daemon::mappers::HarnessSemanticEvent {
            line,
            timestamp_ms: crate::clock::now_millis(),
            record_type: format!("openhuman:{kind}"),
            event: HarnessEvent {
                kind: kind.to_string(),
                payload,
                ..Default::default()
            },
        });
    }

    /// Fold one core progress event and emit whatever it maps to.
    ///
    /// Emits nothing for the progress variants that carry no stream frame; the
    /// watchdog still treats the event as liveness, which is the point of
    /// keeping the two decisions apart.
    pub(super) fn emit_progress(&mut self, progress: &AgentProgress) {
        for (kind, payload) in semantic_events(progress) {
            self.emit(&kind, payload);
        }
    }

    /// How many events this turn has emitted.
    pub(super) fn emitted(&self) -> usize {
        self.emitted
    }
}
