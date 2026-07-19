//! Data model for the `medulla-tinyplace/1` task wire protocol: the frame
//! structs and enums, their trivial serde/inherent `impl`s, and the tolerant
//! `deserialize_with` helpers the [`AgentCapabilities`] derive depends on. The
//! construction and parsing logic lives in the sibling [`encode`](super::encode)
//! and [`decode`](super::decode) modules.

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// Wire version tag stamped on every task frame body.
pub const TINYPLACE_PROTO: &str = "medulla-tinyplace/1";

/// The coding-agent CLI that ran (or should run) a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessProvider {
    /// Anthropic's `claude` CLI harness.
    Claude,
    /// OpenAI's `codex` CLI harness.
    Codex,
    /// The `opencode` CLI harness.
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
    /// A new unit of delegated work.
    Task,
    /// Follow-up input for an in-flight task.
    Input,
    /// A progress update for a running task.
    Status,
    /// A terminal successful result.
    Reply,
    /// A terminal failure.
    Error,
    /// Receipt acknowledgement of a request.
    Ack,
    /// A request asking a peer what it can do.
    Capabilities,
    /// The answer to a `Capabilities` request, carrying [`AgentCapabilities`] JSON.
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

/// Token usage a responder reports for a completed task (child harness
/// consumption, surfaced to the orchestrator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Tokens the child harness consumed as input/prompt.
    #[serde(rename = "inputTokens")]
    pub input_tokens: i64,
    /// Tokens the child harness produced as output/completion.
    #[serde(rename = "outputTokens")]
    pub output_tokens: i64,
}

/// A decoded protocol frame.
///
/// `task_id` is the cycle-scoped correlation key; `correlation_id` (when present)
/// is the globally-unique dispatch key that responders must echo verbatim.
/// `harness` names the provider that ran a task (set on responses); `provider`
/// is an inbound-only hint naming the agent the orchestrator wants to run it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFrame {
    /// Wire version tag ([`TINYPLACE_PROTO`]).
    pub proto: String,
    /// The frame kind.
    pub kind: TaskFrameKind,
    /// Cycle-scoped correlation key.
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// The frame's textual payload (prompt, status, reply, or capabilities JSON).
    pub text: String,
    /// ISO-8601 timestamp supplied by the caller.
    pub ts: String,
    /// Globally-unique dispatch key that responders echo verbatim.
    #[serde(
        rename = "correlationId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub correlation_id: Option<String>,
    /// The provider that ran a task (set on responses).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub harness: Option<HarnessProvider>,
    /// Inbound-only hint naming the agent the orchestrator wants to run this task.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provider: Option<HarnessProvider>,
    /// Inbound-only advisory hint naming the model the orchestrator wants this
    /// task run on (parallels `provider`). The worker daemon may honor it as the
    /// harness `--model`/`-m` or fall back to its configured model; never echoed
    /// on responses.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// Reported on `reply` frames when the child harness surfaced token counts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage: Option<TokenUsage>,
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
    /// The frame kind to build.
    pub kind: TaskFrameKind,
    /// Cycle-scoped correlation key.
    pub task_id: String,
    /// The frame's textual payload.
    pub text: String,
    /// ISO-8601 timestamp supplied by the caller.
    pub ts: String,
    /// Globally-unique dispatch key to echo, when correlating a response.
    pub correlation_id: Option<String>,
    /// The provider that ran a task (set on responses).
    pub harness: Option<HarnessProvider>,
    /// Inbound-only hint naming the agent the orchestrator wants to run this task.
    pub provider: Option<HarnessProvider>,
    /// Inbound-only advisory model hint (parallels `provider`); `None` on the
    /// responses a worker daemon emits.
    pub model: Option<String>,
}

/// What an agent reports it can do, merged with facts its host establishes.
///
/// Field names mirror the TypeScript SDK JSON (camelCase for the multi-word
/// keys), since this object rides inside a `capabilities_result` frame's `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentCapabilities {
    /// The agent's working directory, when reported.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    /// Directories the agent can access.
    #[serde(
        rename = "accessibleDirs",
        default,
        deserialize_with = "de_string_array"
    )]
    pub accessible_dirs: Vec<String>,
    /// The project the agent is working in, when reported.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub project: Option<String>,
    /// The git branch the agent is on, when reported.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch: Option<String>,
    /// Harness providers the agent can run.
    #[serde(default, deserialize_with = "de_providers")]
    pub providers: Vec<HarnessProvider>,
    /// Tool names the agent exposes.
    #[serde(default, deserialize_with = "de_string_array")]
    pub tools: Vec<String>,
    /// MCP server names the agent has configured.
    #[serde(rename = "mcpServers", default, deserialize_with = "de_string_array")]
    pub mcp_servers: Vec<String>,
    /// A free-text summary of the agent, when reported.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub summary: Option<String>,
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
