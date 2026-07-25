//! Receiver-side fold of the SDK's v2 harness stream.
//!
//! A receiver folds each envelope back into live per-session state: which
//! provider it is, what it is doing right now, which tools ran, and a capped
//! human-readable feed. Pure and framework-agnostic — rendering lives elsewhere.
//! v1 envelopes carry no typed events and are ignored here.
//!
//! The envelope/event types come from the published SDK
//! (the SDK `::tinyplace::types` module); this module only derives view state from
//! them. State strings match the SDK's `HarnessSessionState` wire values (see
//! [`crate::tinyplace::status`] for the shared constants).

use ::tinyplace::types::{AnySessionEnvelope, HarnessEventKind, SessionEnvelopeV2};

use super::status::{
    STATE_ERRORED, STATE_IDLE, STATE_RUNNING_TOOL, STATE_STOPPED, STATE_WAITING_APPROVAL,
};

/// Parse a decrypted DM body into either harness envelope version, or `None`
/// when it is not a session envelope. Thin wrapper over
/// [`AnySessionEnvelope::parse`].
pub fn parse_session_envelope(body: &str) -> Option<AnySessionEnvelope> {
    AnySessionEnvelope::parse(body)
}

/// The v2 envelope inside an [`AnySessionEnvelope`], if this is one.
fn as_v2(envelope: &AnySessionEnvelope) -> Option<&SessionEnvelopeV2> {
    match envelope {
        AnySessionEnvelope::V2(env) => Some(env),
        AnySessionEnvelope::V1(_) => None,
    }
}

/// One tool invocation and (once it lands) its result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolActivity {
    pub call_id: String,
    pub tool_name: String,
    /// Normalized tool family (SDK wire string: `shell|file_read|…|other`).
    pub tool_kind: String,
    pub display: String,
    pub started_seq: i64,
    pub done: bool,
    pub ok: Option<bool>,
    pub is_error: Option<bool>,
    pub output_bytes: Option<i64>,
}

/// One entry in the capped human-readable feed.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedEntry {
    pub seq: i64,
    pub ts: String,
    /// Chat-bubble side (`owner` for a user prompt, else `agent`).
    pub role: String,
    pub kind: String,
    pub text: String,
}

/// Live state for a single agent session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionView {
    /// The harness provider wire string (`claude`/`codex`/`opencode`), if seen.
    pub provider: Option<String>,
    pub wrapper_session_id: Option<String>,
    pub harness_session_id: Option<String>,
    pub cwd: Option<String>,
    /// Derived activity state (SDK `HarnessSessionState` wire string).
    pub status: String,
    pub current_task: String,
    pub last_seq: i64,
    pub last_event_id: Option<String>,
    pub last_activity_ts: Option<String>,
    /// Most-recent tool activity, newest last, capped at `limits.tools`.
    pub tools: Vec<ToolActivity>,
    /// Most-recent feed entries, newest last, capped at `limits.feed`.
    pub feed: Vec<FeedEntry>,
}

impl Default for SessionView {
    fn default() -> Self {
        SessionView {
            provider: None,
            wrapper_session_id: None,
            harness_session_id: None,
            cwd: None,
            status: STATE_IDLE.to_string(),
            current_task: "idle".to_string(),
            last_seq: -1,
            last_event_id: None,
            last_activity_ts: None,
            tools: Vec::new(),
            feed: Vec::new(),
        }
    }
}

/// Caps for the retained tool and feed histories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionViewLimits {
    pub tools: usize,
    pub feed: usize,
}

/// The defaults used across the fold: 50 tools, 200 feed entries.
pub const DEFAULT_LIMITS: SessionViewLimits = SessionViewLimits {
    tools: 50,
    feed: 200,
};

/// A fresh, idle session view.
pub fn initial_session_view() -> SessionView {
    SessionView::default()
}

/// Fold one envelope into `view`. Ignores v1 envelopes and out-of-order or
/// duplicate v2 packets (seq must strictly advance). Returns `true` when the
/// view was updated, `false` when it was left unchanged.
pub fn apply_session_envelope(
    view: &mut SessionView,
    envelope: &AnySessionEnvelope,
    limits: SessionViewLimits,
) -> bool {
    let env = match as_v2(envelope) {
        Some(env) => env,
        None => return false,
    };
    if env.event.seq <= view.last_seq {
        return false; // duplicate or out-of-order resend
    }

    if !env.harness.provider.is_empty() {
        view.provider = Some(env.harness.provider.clone());
    }
    view.wrapper_session_id = Some(env.scope.wrapper_session_id.clone());
    view.harness_session_id = Some(env.scope.harness_session_id.clone());
    view.cwd = Some(env.scope.cwd.clone());
    view.last_seq = env.event.seq;
    view.last_event_id = Some(env.event.id.clone());
    view.last_activity_ts = Some(env.event.ts.clone());

    apply_event(view, env, limits);
    true
}

/// Fold a batch of envelopes into a fresh view, applied defensively in seq order.
/// v1 envelopes are dropped.
pub fn fold_session_envelopes(
    envelopes: &[AnySessionEnvelope],
    limits: SessionViewLimits,
) -> SessionView {
    let mut v2: Vec<&SessionEnvelopeV2> = envelopes.iter().filter_map(as_v2).collect();
    v2.sort_by_key(|env| env.event.seq);

    let mut view = initial_session_view();
    for env in v2 {
        apply_session_envelope(&mut view, &AnySessionEnvelope::V2((*env).clone()), limits);
    }
    view
}

fn apply_event(view: &mut SessionView, env: &SessionEnvelopeV2, limits: SessionViewLimits) {
    match env.event.decoded() {
        HarnessEventKind::ToolCall(payload) => {
            view.tools.push(ToolActivity {
                call_id: payload.call_id.clone(),
                tool_name: payload.tool_name.clone(),
                tool_kind: payload.tool_kind.clone(),
                display: payload.display.clone(),
                started_seq: env.event.seq,
                done: false,
                ok: None,
                is_error: None,
                output_bytes: None,
            });
            cap_end(&mut view.tools, limits.tools);
            view.status = STATE_RUNNING_TOOL.to_string();
            view.current_task = format!("{}: {}", payload.tool_name, payload.display);
            push_feed(view, env, payload.display.clone(), limits.feed);
        }
        HarnessEventKind::ToolResult(payload) => {
            if let Some(tool) = view
                .tools
                .iter_mut()
                .find(|t| t.call_id == payload.call_id && !t.done)
            {
                tool.done = true;
                tool.ok = Some(payload.ok);
                tool.is_error = Some(payload.is_error);
                tool.output_bytes = Some(payload.output_bytes);
            }
            let text = if payload.is_error { "error" } else { "ok" };
            push_feed(view, env, text.to_string(), limits.feed);
        }
        HarnessEventKind::Status(payload) => {
            if !payload.state.is_empty() {
                view.status = payload.state;
            }
            view.current_task = payload.detail;
        }
        HarnessEventKind::ApprovalRequest(payload) => {
            view.status = STATE_WAITING_APPROVAL.to_string();
            view.current_task = format!("approval: {}", payload.display);
            push_feed(view, env, payload.display, limits.feed);
        }
        HarnessEventKind::Error(payload) => {
            if payload.fatal {
                view.status = STATE_ERRORED.to_string();
            }
            push_feed(view, env, payload.message, limits.feed);
        }
        HarnessEventKind::Lifecycle(payload) => {
            if payload.phase == "session_end" {
                view.status = STATE_STOPPED.to_string();
                view.current_task = "stopped".to_string();
            }
        }
        HarnessEventKind::UserPrompt(payload) => {
            push_feed(view, env, payload.text, limits.feed);
        }
        HarnessEventKind::AgentMessage(payload) | HarnessEventKind::AgentThinking(payload) => {
            push_feed(view, env, payload.text, limits.feed);
        }
        HarnessEventKind::Unknown(_) => {}
    }
}

fn push_feed(view: &mut SessionView, env: &SessionEnvelopeV2, text: String, cap: usize) {
    view.feed.push(FeedEntry {
        seq: env.event.seq,
        ts: env.event.ts.clone(),
        role: role_of(env),
        kind: env.event.kind.clone(),
        text,
    });
    cap_end(&mut view.feed, cap);
}

/// The chat-bubble side for an event: the wire `role` when present, else derived
/// from the kind (`owner` only for a user prompt).
fn role_of(env: &SessionEnvelopeV2) -> String {
    if !env.event.role.is_empty() {
        return env.event.role.clone();
    }
    if env.event.kind == "user_prompt" {
        "owner".to_string()
    } else {
        "agent".to_string()
    }
}

fn cap_end<T>(items: &mut Vec<T>, cap: usize) {
    if items.len() > cap {
        items.drain(0..items.len() - cap);
    }
}

#[cfg(test)]
#[path = "consumer_tests.rs"]
mod tests;
