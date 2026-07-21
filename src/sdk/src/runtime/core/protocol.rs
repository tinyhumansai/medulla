//! The `medulla-serve` NDJSON frame grammar: parsing inbound serve→host lines
//! into an [`Inbound`] enum, building the outbound host→serve frames, the
//! handshake `hello` params, and folding a [`HarnessEvent`] into [`CoreState`].
//! Pure functions over data — no I/O, which lives in [`super::client`].

use serde_json::{json, Value};

use crate::harness_contract::{HarnessEvent, HarnessState, HarnessStatus, HarnessUsage};
use crate::runtime::CycleResultSummary;
use crate::ui::chat_store::ChatMessage;
use crate::ui::events::TuiEvent;

use super::types::{CoreState, WireError, HOST_PORTS, PROTOCOL_VERSION};

/// A parsed serve→host frame. Only the four serve→host frame kinds are
/// represented (`ready`, `res`, `call`, `event`); host→serve frames (`req`,
/// `ret`, `emit`) are never received. A malformed line yields `None` and is
/// skipped by the caller (serve-protocol §1 "parse-fail is skipped, never fatal").
#[derive(Debug)]
pub(super) enum Inbound {
    /// The handshake banner (serve-protocol §3).
    Ready {
        /// Wire version; the host bails on a mismatch.
        protocol: i64,
        /// The serve build version.
        serve: Option<String>,
        /// The session id serve owns.
        session_id: Option<String>,
        /// Non-null when startup failed; the host treats serve as unavailable.
        error: Option<String>,
    },
    /// A correlated response to a host `req`.
    Res {
        /// The `req` id this answers.
        id: String,
        /// Whether the request succeeded.
        ok: bool,
        /// The success payload (`null` on failure).
        result: Value,
        /// The failure detail, when `ok` is false.
        error: Option<WireError>,
    },
    /// A reverse-RPC port callback (serve→host). Not hosted yet in this
    /// milestone; the driver refuses it `port_unavailable`.
    Call {
        /// The callback id serve minted.
        id: String,
        /// The port name (`inference`, `tools`, …).
        port: String,
    },
    /// An unsolicited event-stream frame (serve-protocol §6).
    Event {
        /// The contiguous per-connection sequence.
        seq: u64,
        /// The wrapped `HarnessEvent` (or serve-level frame) payload.
        event: Value,
    },
}

/// Parse one NDJSON line into an [`Inbound`], or `None` when the line is blank,
/// not an object, or missing/holding an unknown `t` discriminant.
pub(super) fn parse_line(line: &str) -> Option<Inbound> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    let t = value.get("t")?.as_str()?;
    match t {
        "ready" => Some(Inbound::Ready {
            protocol: value.get("protocol").and_then(Value::as_i64).unwrap_or(-1),
            serve: str_field(&value, "serve"),
            session_id: str_field(&value, "sessionId"),
            error: str_field(&value, "error"),
        }),
        "res" => Some(Inbound::Res {
            id: str_field(&value, "id")?,
            ok: value.get("ok").and_then(Value::as_bool).unwrap_or(false),
            result: value.get("result").cloned().unwrap_or(Value::Null),
            error: value
                .get("error")
                .filter(|e| !e.is_null())
                .and_then(|e| serde_json::from_value(e.clone()).ok()),
        }),
        "call" => Some(Inbound::Call {
            id: str_field(&value, "id")?,
            port: str_field(&value, "port").unwrap_or_default(),
        }),
        "event" => Some(Inbound::Event {
            seq: value.get("seq").and_then(Value::as_u64).unwrap_or(0),
            event: value.get("event").cloned().unwrap_or(Value::Null),
        }),
        _ => None,
    }
}

/// Read a string field, returning `None` for absent or JSON-null values.
fn str_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .filter(|v| !v.is_null())
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Serialize a host→serve `req` line (newline-terminated).
pub(super) fn req_line(id: &str, op: &str, params: &Value) -> String {
    let frame = json!({ "t": "req", "id": id, "op": op, "params": params });
    format!("{frame}\n")
}

/// Serialize a host→serve `ret` line refusing an unhosted port `call`.
pub(super) fn port_unavailable_ret(id: &str, port: &str) -> String {
    let frame = json!({
        "t": "ret",
        "id": id,
        "ok": false,
        "error": {
            "code": "port_unavailable",
            "message": format!("host does not yet host the {port} port"),
        },
    });
    format!("{frame}\n")
}

/// Build the `hello` params (serve-protocol §3): the negotiated protocol, host
/// identity, and the ports the host offers to answer.
pub(super) fn hello_params() -> Value {
    json!({
        "protocol": PROTOCOL_VERSION,
        "host": format!("medulla-public/{}", env!("CARGO_PKG_VERSION")),
        "ports": HOST_PORTS,
    })
}

/// Fold one `event.event` payload into `state`, returning `true` when a
/// chat/board-visible change occurred (the caller pings subscribers on `true`).
///
/// Recognised `HarnessEvent` kinds drive the running flag, the task board, and
/// the transcript (user/assistant turns are pushed into
/// [`CoreState::messages`] as they fold, deduplicated against the optimistic
/// echo `submit` already appended); anything else (including serve's
/// `roster_event`) rides through as a passthrough [`TuiEvent`] so no row is
/// silently dropped.
pub(super) fn fold_event(state: &mut CoreState, event: &Value) -> bool {
    match serde_json::from_value::<HarnessEvent>(event.clone()) {
        Ok(HarnessEvent::CycleStart {
            instruction_id,
            cycle_id,
        }) => {
            state.running = true;
            let status = status_mut(state);
            status.state = HarnessState::Running;
            // The instruction has left the FIFO queue to run, so it no longer
            // counts as backlog. Saturating so a cycle_start without a matching
            // instruction_queued (e.g. one seen before the connection attached)
            // can never underflow the depth below zero.
            status.queued = status.queued.saturating_sub(1);
            status.active_instruction_id = Some(instruction_id);
            status.active_cycle_id = Some(cycle_id.clone());
            state.emit(TuiEvent::CycleStart { cycle_id });
            true
        }
        Ok(HarnessEvent::CycleEnd { cycle_id, .. }) => {
            state.running = false;
            let status = status_mut(state);
            status.state = HarnessState::Idle;
            status.active_instruction_id = None;
            status.active_cycle_id = None;
            status.usage.cycles += 1;
            state.last_result = Some(CycleResultSummary::default());
            state.emit(TuiEvent::CycleEnd {
                cycle_id,
                pass_count: 0,
                duration_ms: 0,
            });
            true
        }
        Ok(HarnessEvent::InstructionQueued {
            instruction_id,
            cycle_id,
        }) => {
            let status = status_mut(state);
            status.queued = status.queued.saturating_add(1);
            state.emit(TuiEvent::Unknown {
                kind: "instruction_queued".into(),
                data: serde_json::Map::from_iter([
                    ("instructionId".into(), Value::String(instruction_id)),
                    ("cycleId".into(), Value::String(cycle_id)),
                ]),
            });
            true
        }
        Ok(HarnessEvent::TaskBoardChanged { task }) => {
            let status = status_mut(state);
            match status.tasks.iter_mut().find(|t| t.id == task.id) {
                Some(existing) => *existing = task,
                None => status.tasks.push(task),
            }
            true
        }
        Ok(HarnessEvent::CycleEvent { event: inner }) => {
            // The inner CycleEvent is opaque; TuiEvent's permissive decode keeps
            // any `{kind,...}` shape (unknown kinds ride through verbatim).
            match serde_json::from_value::<TuiEvent>(inner.clone()) {
                Ok(TuiEvent::User { body })
                    if state.pending_user_echo.as_deref() == Some(body.as_str()) =>
                {
                    // The wire echo of the turn `submit` already appended
                    // optimistically (serve-protocol §4.1 `instruct` reflects the
                    // user turn back over the event stream): drop it rather than
                    // double it up in the transcript, mirroring how the backend
                    // runtime's `fold` de-duplicates `pending_user_echo`.
                    state.pending_user_echo = None;
                }
                Ok(tui) => {
                    push_chat_message(state, &tui);
                    state.emit(tui);
                }
                Err(_) => state.emit(passthrough(&inner)),
            }
            true
        }
        // Not a HarnessEvent (e.g. serve's `roster_event`): keep it verbatim.
        Err(_) => {
            state.emit(passthrough(event));
            true
        }
    }
}

/// Append a chat-visible turn into the rendered transcript
/// ([`CoreState::messages`]) as it folds, so [`CoreRuntime::snapshot`]'s
/// `messages`/turn count stay in step with what the event stream shows. Only
/// user/assistant turns render into the transcript; other kinds are no-ops
/// here (they still ride through the general event log via `emit`).
///
/// [`CoreRuntime::snapshot`]: crate::runtime::Runtime::snapshot
fn push_chat_message(state: &mut CoreState, event: &TuiEvent) {
    match event {
        TuiEvent::User { body } => state.messages.push(ChatMessage {
            role: "user".into(),
            content: body.clone(),
        }),
        TuiEvent::Assistant { body } => state.messages.push(ChatMessage {
            role: "assistant".into(),
            content: body.clone(),
        }),
        _ => {}
    }
}

/// Wrap an unrecognized event payload as a [`TuiEvent::Unknown`], preserving its
/// object body (or an empty body when it is not an object).
fn passthrough(event: &Value) -> TuiEvent {
    let kind = event
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let data = event.as_object().cloned().unwrap_or_default();
    TuiEvent::Unknown { kind, data }
}

/// Borrow the harness status, initializing an idle one on first use.
fn status_mut(state: &mut CoreState) -> &mut HarnessStatus {
    state.harness.get_or_insert_with(|| HarnessStatus {
        state: HarnessState::Idle,
        queued: 0,
        active_instruction_id: None,
        active_cycle_id: None,
        tasks: Vec::new(),
        running_delegations: 0,
        usage: HarnessUsage::default(),
        last_result: None,
        escalations: Vec::new(),
    })
}

/// Classify a `ready` frame into the handshake outcome the driver acts on.
pub(super) enum ReadyCheck {
    /// Handshake may proceed; carries the serve version + session id.
    Ok {
        /// The serve build version.
        serve: Option<String>,
        /// The session id serve owns.
        session_id: Option<String>,
    },
    /// A fatal handshake outcome; carries the operator-facing reason.
    Fatal(String),
}

/// Validate a `ready` banner: a non-null `error` or a protocol mismatch is fatal
/// (serve-protocol §3); otherwise the handshake may proceed.
pub(super) fn check_ready(
    protocol: i64,
    serve: Option<String>,
    session_id: Option<String>,
    error: Option<String>,
) -> ReadyCheck {
    if let Some(err) = error {
        return ReadyCheck::Fatal(format!("serve reported startup error: {err}"));
    }
    if protocol != PROTOCOL_VERSION {
        return ReadyCheck::Fatal(format!(
            "protocol mismatch: serve speaks {protocol}, host speaks {PROTOCOL_VERSION}"
        ));
    }
    ReadyCheck::Ok { serve, session_id }
}
