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
pub const STATE_RUNNING_TOOL: &str = "running_tool";
pub const STATE_WAITING_APPROVAL: &str = "waiting_approval";
pub const STATE_IDLE: &str = "idle";
pub const STATE_STOPPED: &str = "stopped";
pub const STATE_ERRORED: &str = "errored";

/// The running state of the status machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStatusState {
    /// `HarnessSessionState` wire string (see the `STATE_*` constants).
    pub state: String,
    pub detail: String,
    pub active_call_id: Option<String>,
    /// Timestamp of the last event that moved the machine (ms since epoch).
    pub last_event_at_ms: i64,
}

/// The result of a reduction/tick: the next state and, when something should be
/// published, the payload to emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusStep {
    pub next: SessionStatusState,
    pub emit: Option<StatusPayload>,
}

/// A semantic event fed to [`reduce_status`]: the typed event kind plus the
/// wall-clock time it occurred (ms since epoch). `None` or `0` means "unknown
/// time", which falls back to the machine's last activity clock.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticEvent {
    pub timestamp_ms: Option<i64>,
    pub event: HarnessEventKind,
}

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

struct Derived {
    state: String,
    detail: String,
    active_call_id: Option<String>,
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
mod tests {
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
}
