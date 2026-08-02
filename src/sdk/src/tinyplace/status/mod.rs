//! Derived session-status state machine for the SDK's v2 harness stream.
//!
//! A transcript records events but never states "the agent is idle". A receiver
//! needs a live "what is it doing right now" signal; this module derives it from
//! the SDK's typed event kinds ([`HarnessEventKind`]), plus a heartbeat/idle tick
//! that keeps the signal honest when no events arrive. Pure and caller-driven —
//! no timers held here. Both entry points return an optional [`StatusPayload`]
//! that should be emitted only when present (change-gated).
//!
//! State strings are the SDK's `HarnessSessionState` wire values, exposed as the
//! `STATE_*` constants so the fold ([`crate::tinyplace::consumer`]) and this machine agree.

use ::tinyplace::types::{HarnessEventKind, StatusPayload};

/// Default idle horizon: age a silent active session after 30s.
pub const DEFAULT_IDLE_AFTER_MS: i64 = 30_000;

const DETAIL_CAP: usize = 120;

/// `HarnessSessionState` wire strings.
pub const STATE_RUNNING: &str = "running";
/// A tool call is currently executing.
pub const STATE_RUNNING_TOOL: &str = "running_tool";
/// Execution is blocked on operator approval.
pub const STATE_WAITING_APPROVAL: &str = "waiting_approval";
/// The session exists but is not actively processing.
pub const STATE_IDLE: &str = "idle";
/// The session ended normally.
pub const STATE_STOPPED: &str = "stopped";
/// The session ended because of a fatal error.
pub const STATE_ERRORED: &str = "errored";

/// Default: the session exists but nothing has happened yet.
pub fn initial_status(now_ms: i64) -> SessionStatusState {
    SessionStatusState {
        state: STATE_IDLE.to_string(),
        detail: "idle".to_string(),
        active_call_id: None,
        last_event_at_ms: now_ms,
    }
}

/// Fold one semantic event into the status machine. Emits a payload only when the
/// derived state, detail, or active call changed.
pub fn reduce_status(prev: &SessionStatusState, event: &SemanticEvent) -> StatusStep {
    let at_ms = time_to_ms(event.timestamp_ms, prev.last_event_at_ms);
    let derived = match derive_from_event(&event.event) {
        Some(derived) => derived,
        None => {
            // The event carries no status signal; keep state but advance the
            // activity clock so heartbeat/idle timing stays honest.
            return StatusStep {
                next: SessionStatusState {
                    last_event_at_ms: at_ms,
                    ..prev.clone()
                },
                emit: None,
            };
        }
    };
    let next = SessionStatusState {
        state: derived.state,
        detail: derived.detail,
        active_call_id: derived.active_call_id,
        last_event_at_ms: at_ms,
    };
    if changed(prev, &next) {
        let emit = Some(to_payload(&next));
        StatusStep { next, emit }
    } else {
        StatusStep { next, emit: None }
    }
}

/// Age a silent session. Once more than `idle_after_ms` has passed since the last
/// event while the machine is active, transition to `idle`. Otherwise, when
/// `heartbeat` is set, re-emit the current status unchanged so downstream "last
/// updated" stays fresh. Emits nothing when neither is due.
pub fn tick_status(
    prev: &SessionStatusState,
    now_ms: i64,
    idle_after_ms: i64,
    heartbeat: bool,
) -> StatusStep {
    let stale = now_ms - prev.last_event_at_ms >= idle_after_ms;
    if stale && is_active(&prev.state) {
        let next = SessionStatusState {
            state: STATE_IDLE.to_string(),
            detail: "idle".to_string(),
            active_call_id: None,
            last_event_at_ms: prev.last_event_at_ms,
        };
        let emit = Some(to_payload(&next));
        return StatusStep { next, emit };
    }
    if heartbeat {
        return StatusStep {
            next: prev.clone(),
            emit: Some(to_payload(prev)),
        };
    }
    StatusStep {
        next: prev.clone(),
        emit: None,
    }
}

fn derive_from_event(event: &HarnessEventKind) -> Option<Derived> {
    match event {
        HarnessEventKind::ToolCall(p) => Some(Derived {
            state: STATE_RUNNING_TOOL.to_string(),
            detail: cap(&format!("running {}: {}", p.tool_name, p.display)),
            active_call_id: non_empty(&p.call_id),
        }),
        HarnessEventKind::ToolResult(_) => Some(Derived {
            state: STATE_RUNNING.to_string(),
            detail: "processing".to_string(),
            active_call_id: None,
        }),
        HarnessEventKind::ApprovalRequest(p) => Some(Derived {
            state: STATE_WAITING_APPROVAL.to_string(),
            detail: cap(&format!("awaiting approval: {}", p.display)),
            active_call_id: p.call_id.clone(),
        }),
        HarnessEventKind::AgentThinking(_) => Some(Derived {
            state: STATE_RUNNING.to_string(),
            detail: "thinking".to_string(),
            active_call_id: None,
        }),
        HarnessEventKind::AgentMessage(_) => Some(Derived {
            state: STATE_RUNNING.to_string(),
            detail: "replying".to_string(),
            active_call_id: None,
        }),
        HarnessEventKind::UserPrompt(_) => Some(Derived {
            state: STATE_RUNNING.to_string(),
            detail: "working".to_string(),
            active_call_id: None,
        }),
        HarnessEventKind::Error(p) => Some(Derived {
            state: if p.fatal {
                STATE_ERRORED
            } else {
                STATE_RUNNING
            }
            .to_string(),
            detail: cap(&p.message),
            active_call_id: None,
        }),
        HarnessEventKind::Lifecycle(p) => lifecycle_status(&p.phase),
        HarnessEventKind::Status(p) => Some(Derived {
            state: p.state.clone(),
            detail: p.detail.clone(),
            active_call_id: p.active_call_id.clone(),
        }),
        HarnessEventKind::Unknown(_) => None,
    }
}

fn lifecycle_status(phase: &str) -> Option<Derived> {
    match phase {
        "session_start" | "turn_start" => Some(Derived {
            state: STATE_RUNNING.to_string(),
            detail: "working".to_string(),
            active_call_id: None,
        }),
        "turn_end" => Some(Derived {
            state: STATE_IDLE.to_string(),
            detail: "idle".to_string(),
            active_call_id: None,
        }),
        "compact" => Some(Derived {
            state: STATE_RUNNING.to_string(),
            detail: "compacting".to_string(),
            active_call_id: None,
        }),
        "session_end" => Some(Derived {
            state: STATE_STOPPED.to_string(),
            detail: "stopped".to_string(),
            active_call_id: None,
        }),
        _ => None,
    }
}

fn to_payload(state: &SessionStatusState) -> StatusPayload {
    StatusPayload {
        state: state.state.clone(),
        detail: state.detail.clone(),
        active_call_id: state.active_call_id.clone(),
    }
}

fn changed(a: &SessionStatusState, b: &SessionStatusState) -> bool {
    a.state != b.state || a.detail != b.detail || a.active_call_id != b.active_call_id
}

fn is_active(state: &str) -> bool {
    matches!(
        state,
        STATE_RUNNING | STATE_RUNNING_TOOL | STATE_WAITING_APPROVAL
    )
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn cap(value: &str) -> String {
    let line = value.split('\n').next().unwrap_or(value);
    if line.chars().count() > DETAIL_CAP {
        let truncated: String = line.chars().take(DETAIL_CAP - 1).collect();
        format!("{truncated}…")
    } else {
        line.to_string()
    }
}

fn time_to_ms(timestamp_ms: Option<i64>, fallback: i64) -> i64 {
    match timestamp_ms {
        Some(ms) if ms != 0 => ms,
        _ => fallback,
    }
}

#[cfg(test)]
mod tests;

mod types;
use types::Derived;
pub use types::SemanticEvent;
pub use types::SessionStatusState;
pub use types::StatusStep;
