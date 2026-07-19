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
mod tests {
    use crate::tinyplace::{
        apply_session_envelope, fold_session_envelopes, initial_session_view,
        parse_session_envelope, AnySessionEnvelope, SessionViewLimits, DEFAULT_LIMITS,
        SESSION_ENVELOPE_VERSION_V2, STATE_ERRORED, STATE_IDLE, STATE_RUNNING, STATE_RUNNING_TOOL,
        STATE_STOPPED, STATE_WAITING_APPROVAL,
    };
    use serde_json::{json, Value};

    /// Build a v2 envelope with the given `event` object (kind/role/payload merged
    /// with id/seq/ts). Envelopes come from the SDK via `parse_session_envelope`.
    fn v2_envelope(seq: i64, event_body: Value) -> AnySessionEnvelope {
        let mut event = json!({ "id": format!("id-{seq}"), "seq": seq, "ts": format!("ts-{seq}") });
        let obj = event.as_object_mut().unwrap();
        for (k, val) in event_body.as_object().unwrap() {
            obj.insert(k.clone(), val.clone());
        }
        let body = json!({
            "envelope_version": SESSION_ENVELOPE_VERSION_V2,
            "version": 2,
            "bucket": { "unit": "minute", "start": "s", "end": "e" },
            "scope": {
                "type": "session", "key": "k", "cwd": "/repo",
                "wrapper_session_id": "wsid-1", "harness_session_id": "hsid-1",
            },
            "harness": { "provider": "claude", "command": "claude", "argv": ["-p"] },
            "event": event,
            "source": { "path": "/t.jsonl", "record_type": "jsonl" },
        })
        .to_string();
        parse_session_envelope(&body).unwrap()
    }

    fn tool_call(seq: i64, call_id: &str, name: &str, display: &str) -> AnySessionEnvelope {
        v2_envelope(
            seq,
            json!({"kind":"tool_call","role":"agent","payload":{
            "call_id":call_id,"tool_name":name,"tool_kind":"shell","display":display,"input":{}}}),
        )
    }

    fn tool_result(seq: i64, call_id: &str, is_error: bool) -> AnySessionEnvelope {
        v2_envelope(
            seq,
            json!({"kind":"tool_result","role":"agent","payload":{
            "call_id":call_id,"ok":!is_error,"is_error":is_error,"output":"o","output_bytes":1}}),
        )
    }

    #[test]
    fn initial_view_is_idle() {
        let view = initial_session_view();
        assert_eq!(view.status, STATE_IDLE);
        assert_eq!(view.current_task, "idle");
        assert_eq!(view.last_seq, -1);
        assert!(view.tools.is_empty());
        assert!(view.feed.is_empty());
    }

    #[test]
    fn applies_a_tool_call_then_result() {
        let mut view = initial_session_view();
        assert!(apply_session_envelope(
            &mut view,
            &tool_call(0, "c1", "Bash", "npm test"),
            DEFAULT_LIMITS
        ));
        assert_eq!(view.provider.as_deref(), Some("claude"));
        assert_eq!(view.cwd.as_deref(), Some("/repo"));
        assert_eq!(view.status, STATE_RUNNING_TOOL);
        assert_eq!(view.current_task, "Bash: npm test");
        assert_eq!(view.last_seq, 0);
        assert_eq!(view.tools.len(), 1);
        assert!(!view.tools[0].done);
        assert_eq!(view.feed.len(), 1);
        assert_eq!(view.feed[0].text, "npm test");

        assert!(apply_session_envelope(
            &mut view,
            &tool_result(1, "c1", false),
            DEFAULT_LIMITS
        ));
        assert!(view.tools[0].done);
        assert_eq!(view.tools[0].ok, Some(true));
        assert_eq!(view.tools[0].is_error, Some(false));
        assert_eq!(view.tools[0].output_bytes, Some(1));
        assert_eq!(view.feed.last().unwrap().text, "ok");
    }

    #[test]
    fn out_of_order_and_duplicate_packets_are_ignored() {
        let mut view = initial_session_view();
        assert!(apply_session_envelope(
            &mut view,
            &tool_call(5, "c1", "Bash", "x"),
            DEFAULT_LIMITS
        ));
        // Same seq — duplicate.
        assert!(!apply_session_envelope(
            &mut view,
            &tool_call(5, "c2", "Bash", "y"),
            DEFAULT_LIMITS
        ));
        // Lower seq — out of order.
        assert!(!apply_session_envelope(
            &mut view,
            &tool_call(3, "c3", "Bash", "z"),
            DEFAULT_LIMITS
        ));
        assert_eq!(view.last_seq, 5);
        assert_eq!(view.tools.len(), 1);
    }

    #[test]
    fn v1_envelopes_are_ignored() {
        let mut view = initial_session_view();
        let body = json!({
            "envelope_version": "tinyplace.harness.session.v1",
            "version": 1,
            "bucket": { "unit": "hour", "start": "s", "end": "e" },
            "scope": { "type": "folder", "key": "k", "cwd": "/r",
                "wrapper_session_id": "w", "harness_session_id": "h" },
            "harness": { "provider": "codex", "command": "codex", "argv": [] },
            "message": { "id": "m", "line": 1, "role": "agent", "text": "hi", "timestamp": "t" },
            "source": { "path": "/p", "record_type": "x" },
        })
        .to_string();
        let env = parse_session_envelope(&body).unwrap();
        assert!(matches!(env, AnySessionEnvelope::V1(_)));
        assert!(!apply_session_envelope(&mut view, &env, DEFAULT_LIMITS));
        assert_eq!(view.last_seq, -1);
    }

    #[test]
    fn status_and_approval_and_error_update_state() {
        let mut view = initial_session_view();
        apply_session_envelope(
            &mut view,
            &v2_envelope(
                0,
                json!({"kind":"status","role":"agent","payload":{"state":"running","detail":"thinking"}}),
            ),
            DEFAULT_LIMITS,
        );
        assert_eq!(view.status, STATE_RUNNING);
        assert_eq!(view.current_task, "thinking");

        apply_session_envelope(
            &mut view,
            &v2_envelope(
                1,
                json!({"kind":"approval_request","role":"agent","payload":{"tool_name":"Bash","display":"rm -rf"}}),
            ),
            DEFAULT_LIMITS,
        );
        assert_eq!(view.status, STATE_WAITING_APPROVAL);
        assert_eq!(view.current_task, "approval: rm -rf");

        apply_session_envelope(
            &mut view,
            &v2_envelope(
                2,
                json!({"kind":"error","role":"agent","payload":{"message":"fatal boom","fatal":true}}),
            ),
            DEFAULT_LIMITS,
        );
        assert_eq!(view.status, STATE_ERRORED);

        apply_session_envelope(
            &mut view,
            &v2_envelope(
                3,
                json!({"kind":"lifecycle","role":"agent","payload":{"phase":"session_end"}}),
            ),
            DEFAULT_LIMITS,
        );
        assert_eq!(view.status, STATE_STOPPED);
        assert_eq!(view.current_task, "stopped");
    }

    #[test]
    fn caps_tools_and_feed() {
        let limits = SessionViewLimits { tools: 3, feed: 4 };
        let mut view = initial_session_view();
        for seq in 0..10 {
            apply_session_envelope(
                &mut view,
                &tool_call(seq, &format!("c{seq}"), "Bash", &format!("cmd-{seq}")),
                limits,
            );
        }
        assert_eq!(view.tools.len(), 3);
        // Newest last: the final three calls survive.
        assert_eq!(view.tools[0].display, "cmd-7");
        assert_eq!(view.tools[2].display, "cmd-9");
        assert_eq!(view.feed.len(), 4);
        assert_eq!(view.feed[3].text, "cmd-9");
    }

    #[test]
    fn user_prompt_feed_entry_is_owner_role() {
        let mut view = initial_session_view();
        apply_session_envelope(
            &mut view,
            &v2_envelope(
                0,
                json!({"kind":"user_prompt","role":"owner","payload":{"text":"do it","source":"human"}}),
            ),
            DEFAULT_LIMITS,
        );
        assert_eq!(view.feed.len(), 1);
        assert_eq!(view.feed[0].role, "owner");
        assert_eq!(view.feed[0].kind, "user_prompt");
        assert_eq!(view.feed[0].text, "do it");
    }

    #[test]
    fn fold_sorts_by_seq_defensively() {
        let envelopes = vec![
            tool_call(2, "c2", "Bash", "second"),
            tool_call(0, "c0", "Bash", "zeroth"),
            tool_result(3, "c2", false),
            tool_call(1, "c1", "Bash", "first"),
        ];
        let view = fold_session_envelopes(&envelopes, DEFAULT_LIMITS);
        assert_eq!(view.last_seq, 3);
        assert_eq!(view.tools.len(), 3);
        assert_eq!(view.tools[0].display, "zeroth");
        assert_eq!(view.tools[1].display, "first");
        assert_eq!(view.tools[2].display, "second");
        // The result landed on the seq-2 call.
        assert!(view.tools[2].done);
    }
}
