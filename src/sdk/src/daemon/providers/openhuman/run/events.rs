//! The turn's outgoing event stream: the behaviour of the [`super::types::EventSink`].
//!
//! The sink's shape lives in [`super::types`]; this module implements it —
//! every emission advances the line, bumps the count, and (when there is one)
//! reaches the callback. The counter is shared with [`super`]'s executor so a
//! turn can report how many events it produced without the observer having
//! recorded them in step.

use serde_json::Value;

use crate::protocol::HarnessEvent;

use super::super::super::types::OnEvent;
use super::core_contract::AgentProgress;
use super::types::EventSink;

impl EventSink {
    /// A sink feeding `on_event`, or counting silently when it is `None`.
    pub(super) fn new(on_event: Option<OnEvent>) -> Self {
        Self {
            on_event,
            emitted: 0,
            fold: Default::default(),
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

    /// Fold one core progress event and emit whatever it completes.
    ///
    /// Deltas are held by the fold until a phase boundary, so a chatty turn
    /// emits whole messages and reasoning snapshots rather than one event per
    /// token — the bounded transcript keeps the end of the turn (the reply and
    /// the last tool calls) instead of exhausting its cap on a streamed
    /// paragraph. The watchdog still credits every event as liveness, which is
    /// the point of keeping the two decisions apart.
    pub(super) fn emit_progress(&mut self, progress: &AgentProgress) {
        for (kind, payload) in self.fold.fold(progress) {
            self.emit(&kind, payload);
        }
    }

    /// How many events this turn has emitted.
    pub(super) fn emitted(&self) -> usize {
        self.emitted
    }
}
