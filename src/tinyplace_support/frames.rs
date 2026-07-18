//! The `medulla-tinyplace/1` task wire protocol.
//!
//! An orchestrator delegates work to remote coding agents over an encrypted
//! transport using a small JSON frame. Peers exchange `task`/`input` requests
//! and answer with `ack`/`status`/`reply`/`error`. A `capabilities` frame is the
//! request ("what can you do here?"); the answer is a distinct
//! `capabilities_result` frame carrying [`AgentCapabilities`] JSON in `text`, so
//! a result is never mistaken for a new request. Frames correlate by `task_id`
//! (cycle-scoped) and, when present, an opaque `correlation_id` (globally unique
//! per dispatch) that responders echo back verbatim.

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// Wire version tag stamped on every task frame body.
pub const TINYPLACE_PROTO: &str = "medulla-tinyplace/1";

/// The coding-agent CLI that ran (or should run) a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessProvider {
    Claude,
    Codex,
    Opencode,
}

impl HarnessProvider {
    /// The wire string for this provider.
    pub fn as_str(&self) -> &'static str {
        match self {
            HarnessProvider::Claude => "claude",
            HarnessProvider::Codex => "codex",
            HarnessProvider::Opencode => "opencode",
        }
    }

    /// Parse a provider name, returning `None` for anything unrecognized.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(HarnessProvider::Claude),
            "codex" => Some(HarnessProvider::Codex),
            "opencode" => Some(HarnessProvider::Opencode),
            _ => None,
        }
    }
}

/// The frame kinds the daemon and orchestrator loop exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskFrameKind {
    Task,
    Input,
    Status,
    Reply,
    Error,
    Ack,
    Capabilities,
    CapabilitiesResult,
}

impl TaskFrameKind {
    /// The wire string for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskFrameKind::Task => "task",
            TaskFrameKind::Input => "input",
            TaskFrameKind::Status => "status",
            TaskFrameKind::Reply => "reply",
            TaskFrameKind::Error => "error",
            TaskFrameKind::Ack => "ack",
            TaskFrameKind::Capabilities => "capabilities",
            TaskFrameKind::CapabilitiesResult => "capabilities_result",
        }
    }

    /// Parse a kind name, returning `None` for anything unrecognized.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "task" => Some(TaskFrameKind::Task),
            "input" => Some(TaskFrameKind::Input),
            "status" => Some(TaskFrameKind::Status),
            "reply" => Some(TaskFrameKind::Reply),
            "error" => Some(TaskFrameKind::Error),
            "ack" => Some(TaskFrameKind::Ack),
            "capabilities" => Some(TaskFrameKind::Capabilities),
            "capabilities_result" => Some(TaskFrameKind::CapabilitiesResult),
            _ => None,
        }
    }
}

/// A decoded protocol frame.
///
/// `task_id` is the cycle-scoped correlation key; `correlation_id` (when present)
/// is the globally-unique dispatch key that responders must echo verbatim.
/// `harness` names the provider that ran a task (set on responses); `provider`
/// is an inbound-only hint naming the agent the orchestrator wants to run it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFrame {
    pub proto: String,
    pub kind: TaskFrameKind,
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub text: String,
    pub ts: String,
    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none", default)]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub harness: Option<HarnessProvider>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provider: Option<HarnessProvider>,
}

impl TaskFrame {
    /// Serialize this frame for an encrypted message body.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("TaskFrame always serializes")
    }
}

/// Fields needed to build and serialize a task frame. `ts` is supplied by the
/// caller (an ISO-8601 timestamp) so this crate stays free of a clock dependency.
#[derive(Debug, Clone)]
pub struct EncodeFrameInput {
    pub kind: TaskFrameKind,
    pub task_id: String,
    pub text: String,
    pub ts: String,
    pub correlation_id: Option<String>,
    pub harness: Option<HarnessProvider>,
    pub provider: Option<HarnessProvider>,
}

/// Build and serialize a task frame body.
pub fn encode_task_frame(input: EncodeFrameInput) -> String {
    TaskFrame {
        proto: TINYPLACE_PROTO.to_string(),
        kind: input.kind,
        task_id: input.task_id,
        text: input.text,
        ts: input.ts,
        correlation_id: input.correlation_id,
        harness: input.harness,
        provider: input.provider,
    }
    .encode()
}

/// Parse a decrypted body into a [`TaskFrame`], or `None` when it is not one of
/// ours (plain chatter, another protocol, or a malformed frame). Never panics.
pub fn decode_task_frame(body: &str) -> Option<TaskFrame> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = value.as_object()?;

    if obj.get("proto").and_then(|v| v.as_str()) != Some(TINYPLACE_PROTO) {
        return None;
    }
    let kind = TaskFrameKind::from_wire(obj.get("kind").and_then(|v| v.as_str())?)?;
    let task_id = obj.get("taskId").and_then(|v| v.as_str())?.to_string();
    let text = obj.get("text").and_then(|v| v.as_str())?.to_string();
    // Missing/non-string ts is tolerated: encoders always stamp it, but a peer
    // that drops it should not sink the whole frame.
    let ts = obj
        .get("ts")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let correlation_id = obj
        .get("correlationId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let harness = obj
        .get("harness")
        .and_then(|v| v.as_str())
        .and_then(HarnessProvider::from_wire);
    let provider = obj
        .get("provider")
        .and_then(|v| v.as_str())
        .and_then(HarnessProvider::from_wire);

    Some(TaskFrame {
        proto: TINYPLACE_PROTO.to_string(),
        kind,
        task_id,
        text,
        ts,
        correlation_id,
        harness,
        provider,
    })
}

/// What an agent reports it can do, merged with facts its host establishes.
///
/// Field names mirror the TypeScript SDK JSON (camelCase for the multi-word
/// keys), since this object rides inside a `capabilities_result` frame's `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentCapabilities {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    #[serde(
        rename = "accessibleDirs",
        default,
        deserialize_with = "de_string_array"
    )]
    pub accessible_dirs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch: Option<String>,
    #[serde(default, deserialize_with = "de_providers")]
    pub providers: Vec<HarnessProvider>,
    #[serde(default, deserialize_with = "de_string_array")]
    pub tools: Vec<String>,
    #[serde(rename = "mcpServers", default, deserialize_with = "de_string_array")]
    pub mcp_servers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub summary: Option<String>,
}

/// Parse an [`AgentCapabilities`] object from the frame `text` payload. Tolerant
/// of unknown providers and non-string array entries (both are dropped) and
/// never panics; returns `None` only when `text` is not a JSON object.
pub fn parse_agent_capabilities(text: &str) -> Option<AgentCapabilities> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if !value.is_object() {
        return None;
    }
    serde_json::from_value(value).ok()
}

/// Deserialize a `Vec<String>`, discarding non-string and blank entries.
fn de_string_array<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ArrayVisitor;
    impl<'de> Visitor<'de> for ArrayVisitor {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an array of strings")
        }
        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut out = Vec::new();
            while let Some(item) = seq.next_element::<serde_json::Value>()? {
                if let Some(s) = item.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        out.push(trimmed.to_string());
                    }
                }
            }
            Ok(out)
        }
    }
    deserializer.deserialize_any(ArrayVisitor)
}

/// Deserialize a provider list, dropping any unrecognized entries.
fn de_providers<'de, D>(deserializer: D) -> Result<Vec<HarnessProvider>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = de_string_array(deserializer)?;
    Ok(raw
        .iter()
        .filter_map(|s| HarnessProvider::from_wire(s))
        .collect())
}

#[cfg(test)]
mod tests {
use serde_json::json;
use crate::tinyplace_support::{
    decode_task_frame, encode_task_frame, parse_agent_capabilities, EncodeFrameInput,
    HarnessProvider, TaskFrameKind, TINYPLACE_PROTO,
};

#[test]
fn encodes_a_minimal_frame() {
    let body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: "cycle-1".to_string(),
        text: "do the thing".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: None,
        harness: None,
        provider: None,
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["proto"], TINYPLACE_PROTO);
    assert_eq!(value["kind"], "task");
    assert_eq!(value["taskId"], "cycle-1");
    assert_eq!(value["text"], "do the thing");
    assert_eq!(value["ts"], "2026-07-18T00:00:00.000Z");
    // Optional fields are omitted when absent.
    assert!(value.get("correlationId").is_none());
    assert!(value.get("harness").is_none());
    assert!(value.get("provider").is_none());
}

#[test]
fn encodes_optional_fields_when_present() {
    let body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::CapabilitiesResult,
        task_id: "t".to_string(),
        text: "{}".to_string(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: Some("corr-9".to_string()),
        harness: Some(HarnessProvider::Codex),
        provider: Some(HarnessProvider::Claude),
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["kind"], "capabilities_result");
    assert_eq!(value["correlationId"], "corr-9");
    assert_eq!(value["harness"], "codex");
    assert_eq!(value["provider"], "claude");
}

#[test]
fn round_trips_every_kind() {
    for (kind, wire) in [
        (TaskFrameKind::Task, "task"),
        (TaskFrameKind::Input, "input"),
        (TaskFrameKind::Status, "status"),
        (TaskFrameKind::Reply, "reply"),
        (TaskFrameKind::Error, "error"),
        (TaskFrameKind::Ack, "ack"),
        (TaskFrameKind::Capabilities, "capabilities"),
        (TaskFrameKind::CapabilitiesResult, "capabilities_result"),
    ] {
        let body = encode_task_frame(EncodeFrameInput {
            kind,
            task_id: "t".to_string(),
            text: "x".to_string(),
            ts: "ts".to_string(),
            correlation_id: None,
            harness: None,
            provider: None,
        });
        let decoded = decode_task_frame(&body).expect("valid frame decodes");
        assert_eq!(decoded.kind, kind);
        assert_eq!(decoded.kind.as_str(), wire);
    }
}

#[test]
fn decodes_a_full_frame() {
    let body = json!({
        "proto": TINYPLACE_PROTO,
        "kind": "reply",
        "taskId": "cycle-7",
        "text": "done",
        "ts": "2026-07-18T00:00:00.000Z",
        "correlationId": "corr-1",
        "harness": "opencode",
        "provider": "claude",
    })
    .to_string();
    let frame = decode_task_frame(&body).unwrap();
    assert_eq!(frame.kind, TaskFrameKind::Reply);
    assert_eq!(frame.task_id, "cycle-7");
    assert_eq!(frame.correlation_id.as_deref(), Some("corr-1"));
    assert_eq!(frame.harness, Some(HarnessProvider::Opencode));
    assert_eq!(frame.provider, Some(HarnessProvider::Claude));
}

#[test]
fn decode_tolerates_missing_ts() {
    let body = json!({
        "proto": TINYPLACE_PROTO,
        "kind": "ack",
        "taskId": "t",
        "text": "",
    })
    .to_string();
    let frame = decode_task_frame(&body).unwrap();
    assert_eq!(frame.ts, "");
}

#[test]
fn decode_drops_unknown_provider_without_failing() {
    let body = json!({
        "proto": TINYPLACE_PROTO,
        "kind": "task",
        "taskId": "t",
        "text": "x",
        "ts": "ts",
        "provider": "gemini",
    })
    .to_string();
    let frame = decode_task_frame(&body).unwrap();
    assert_eq!(frame.provider, None);
}

#[test]
fn decode_rejects_non_frames() {
    assert!(decode_task_frame("not json").is_none());
    assert!(decode_task_frame("42").is_none());
    assert!(decode_task_frame(r#"{"hello":"world"}"#).is_none());
    // Wrong proto tag.
    assert!(decode_task_frame(r#"{"proto":"other/1","kind":"task","taskId":"t","text":"x"}"#)
        .is_none());
    // Unknown kind.
    assert!(decode_task_frame(
        &json!({"proto": TINYPLACE_PROTO, "kind": "nope", "taskId": "t", "text": "x"}).to_string()
    )
    .is_none());
    // Missing required text.
    assert!(decode_task_frame(
        &json!({"proto": TINYPLACE_PROTO, "kind": "task", "taskId": "t"}).to_string()
    )
    .is_none());
}

#[test]
fn parses_agent_capabilities() {
    let text = json!({
        "cwd": "/repo",
        "accessibleDirs": ["/repo", "/tmp", "", "  "],
        "project": "medulla",
        "branch": "main",
        "providers": ["claude", "codex", "gemini"],
        "tools": ["Bash", "Read"],
        "mcpServers": ["langfuse"],
        "summary": "coding agent",
    })
    .to_string();
    let caps = parse_agent_capabilities(&text).unwrap();
    assert_eq!(caps.cwd.as_deref(), Some("/repo"));
    // Blank entries dropped, real ones trimmed/kept.
    assert_eq!(caps.accessible_dirs, vec!["/repo", "/tmp"]);
    assert_eq!(caps.project.as_deref(), Some("medulla"));
    // Unknown providers filtered out.
    assert_eq!(
        caps.providers,
        vec![HarnessProvider::Claude, HarnessProvider::Codex]
    );
    assert_eq!(caps.tools, vec!["Bash", "Read"]);
    assert_eq!(caps.mcp_servers, vec!["langfuse"]);
    assert_eq!(caps.summary.as_deref(), Some("coding agent"));
}

#[test]
fn parse_agent_capabilities_defaults_missing_arrays() {
    let caps = parse_agent_capabilities(r#"{"cwd":"/x"}"#).unwrap();
    assert!(caps.accessible_dirs.is_empty());
    assert!(caps.providers.is_empty());
    assert!(caps.tools.is_empty());
    assert!(caps.mcp_servers.is_empty());
    assert!(parse_agent_capabilities("[]").is_none());
    assert!(parse_agent_capabilities("nope").is_none());
}
}
