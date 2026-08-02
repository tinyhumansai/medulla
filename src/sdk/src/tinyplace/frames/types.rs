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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

    /// The human-readable product name, for UI labels.
    ///
    /// Distinct from [`HarnessProvider::as_str`], which is the lowercase wire
    /// identifier and must not change.
    pub fn display_name(&self) -> &'static str {
        match self {
            HarnessProvider::Claude => "Claude Code",
            HarnessProvider::Codex => "Codex",
            HarnessProvider::Opencode => "OpenCode",
        }
    }

    /// A single-column glyph standing in for the provider name.
    ///
    /// For the status line's `icon` harness style: an operator running one
    /// provider does not need its name spelled on every row, and the columns it
    /// costs are the ones the working directory wants. Deliberately geometric
    /// rather than emoji — an emoji is two columns wide on some terminals and
    /// one on others, which would break the rail's width arithmetic.
    pub fn icon(&self) -> &'static str {
        match self {
            HarnessProvider::Claude => "✳",
            HarnessProvider::Codex => "◆",
            HarnessProvider::Opencode => "◻",
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
    /// Stop an in-flight task. Sent when the requester has given up waiting, so
    /// the responder is not left running work nobody will read — and, because a
    /// responder refuses a second task with a live id, so that id is freed.
    Abort,
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
    /// A lightweight request for CPU, memory, and IP details.
    SystemInfo,
    /// The answer to a `SystemInfo` request, carrying worker system-info JSON.
    SystemInfoResult,
}

impl TaskFrameKind {
    /// The wire string for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskFrameKind::Task => "task",
            TaskFrameKind::Input => "input",
            TaskFrameKind::Abort => "abort",
            TaskFrameKind::Status => "status",
            TaskFrameKind::Reply => "reply",
            TaskFrameKind::Error => "error",
            TaskFrameKind::Ack => "ack",
            TaskFrameKind::Capabilities => "capabilities",
            TaskFrameKind::CapabilitiesResult => "capabilities_result",
            TaskFrameKind::SystemInfo => "system_info",
            TaskFrameKind::SystemInfoResult => "system_info_result",
        }
    }

    /// Parse a kind name, returning `None` for anything unrecognized.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "task" => Some(TaskFrameKind::Task),
            "input" => Some(TaskFrameKind::Input),
            "abort" => Some(TaskFrameKind::Abort),
            "status" => Some(TaskFrameKind::Status),
            "reply" => Some(TaskFrameKind::Reply),
            "error" => Some(TaskFrameKind::Error),
            "ack" => Some(TaskFrameKind::Ack),
            "capabilities" => Some(TaskFrameKind::Capabilities),
            "capabilities_result" => Some(TaskFrameKind::CapabilitiesResult),
            "system_info" => Some(TaskFrameKind::SystemInfo),
            "system_info_result" => Some(TaskFrameKind::SystemInfoResult),
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

/// The metering window a paid harness seat renews its allowance on.
///
/// Wire values are snake_case (`five_hour`), matching the provider enum. Defaults
/// to [`BudgetWindow::Unknown`] so a probe that cannot determine the window still
/// serializes a well-formed descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BudgetWindow {
    /// A per-day allowance.
    Daily,
    /// A per-week allowance.
    Weekly,
    /// The rolling five-hour window some subscriptions meter on.
    FiveHour,
    /// The metering window is not known.
    #[default]
    Unknown,
}

/// How a [`HarnessBudget`]'s numbers were arrived at.
///
/// `estimate` is the expected common case: providers rarely publish exact
/// per-window numbers, so most descriptors are best-effort inferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetSource {
    /// Best-effort inference; no exact per-window numbers were published.
    Estimate,
    /// The provider itself reported the numbers.
    ProviderReported,
    /// The operator configured the numbers explicitly.
    Configured,
}

/// A best-effort, per-harness token-budget descriptor advertised on
/// `capabilities_result` so an orchestrator can size tasks against real
/// capacity.
///
/// Every numeric field is optional — a probe that cannot establish a number
/// leaves it absent rather than inventing one. Budget accounting is *soft*: a
/// missing or stale descriptor must fail open and never block delegation, and a
/// mis-reported budget cannot push a consumer past a limit the provider still
/// enforces. `seat` is an opaque identifier only; API keys and credential
/// material are never resolved into this descriptor and never appear in a frame
/// or diagnostic.
///
/// Multi-word field names are camelCase on the wire (`limitTokens`), matching the
/// rest of this frame module; enum values are snake_case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessBudget {
    /// The provider this budget describes.
    pub provider: HarnessProvider,
    /// Opaque identifier for the paid seat/subscription the harness draws from,
    /// when known. Never a credential; omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seat: Option<String>,
    /// The metering window the allowance renews on.
    pub window: BudgetWindow,
    /// Estimated allowance for the window, or absent when unknown.
    #[serde(
        rename = "limitTokens",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub limit_tokens: Option<i64>,
    /// Best-effort consumption so far in the window, or absent when unknown.
    #[serde(
        rename = "usedTokens",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub used_tokens: Option<i64>,
    /// `limit - used`, clamped at zero; absent unless both inputs are known.
    #[serde(
        rename = "remainingTokens",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub remaining_tokens: Option<i64>,
    /// Unix seconds until which the seat is throttled/parked, else absent.
    #[serde(
        rename = "cooldownUntil",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub cooldown_until: Option<i64>,
    /// How the numbers were arrived at.
    pub source: BudgetSource,
}

/// Per-provider readiness advertised alongside budgets.
///
/// A harness that is installed but currently unusable — unauthenticated,
/// unreachable, or cooling down — is reported present with `ready = false` and a
/// `reason`, rather than omitted, so a consumer does not route work that will
/// fail. Readiness is heuristic and fails open: an unverifiable fact leaves the
/// harness `ready` rather than forcing it false.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessReadiness {
    /// The provider this readiness describes.
    pub provider: HarnessProvider,
    /// Whether the harness is believed usable right now.
    pub ready: bool,
    /// Why the harness is not ready, when `ready` is false; omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

/// A decoded protocol frame.
///
/// `task_id` is the cycle-scoped correlation key; `correlation_id` (when present)
/// is the globally-unique dispatch key that responders must echo verbatim.
/// `harness` names the provider that ran a task (set on responses); `provider`
/// is an inbound-only hint naming the agent the orchestrator wants to run it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Inbound-only named custom harness preset to run on the selected host.
    ///
    /// Additive to `provider`: older peers ignore it, while a current worker
    /// resolves it to its configured base CLI, model, and OpenRouter route.
    #[serde(
        rename = "customHarness",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub custom_harness: Option<Box<str>>,
    /// Inbound-only advisory hint naming the model the orchestrator wants this
    /// task run on (parallels `provider`). The worker daemon may honor it as the
    /// harness `--model`/`-m` or fall back to its configured model; never echoed
    /// on responses.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// Inbound-only: run this installed *workflow* instead of handing `text` to
    /// a harness as an instruction.
    ///
    /// A workflow is a saved multi-step graph, so naming one turns a single task
    /// frame into a whole plan the worker executes, dispatching each `agent` node
    /// to its own harness. `text` becomes the trigger payload (JSON, or a bare
    /// string the workflow can read as `=run.trigger.text`). Additive and
    /// optional in both directions: a peer that predates workflows omits the key,
    /// and a worker without the workflow feature replies with an error naming the
    /// id it could not find.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workflow: Option<String>,
    /// Inbound-only: which slice of the workflow tools this task's harness is
    /// served.
    ///
    /// Absent — every ordinary task — means the full authoring surface. The one
    /// sender that sets it is an evolution pass, which asks for `"propose"` so
    /// the review turn is served no verb that writes or runs a graph. Carried
    /// per task rather than configured per worker because the same worker
    /// dispatches authoring turns and review turns minutes apart.
    ///
    /// Additive and optional in both directions: a peer that predates this
    /// omits the key, and a worker that does not understand it serves its usual
    /// tools — which is the pre-existing behaviour, not a new hazard.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_mode: Option<String>,
    /// Inbound-only: the continuity group this task belongs to, when the sender
    /// wants successive tasks to share one harness session.
    ///
    /// Absent by default, and that default is load-bearing. A task frame is
    /// discrete work, so two tasks must never see each other's context — which
    /// is why [`route_session_class`] routes a task
    /// [`Bounded`](crate::sessions::SessionClass::Bounded) and the worker
    /// resumes nothing. Naming a conversation is how a sender says *these* tasks
    /// are one conversation and should remember each other.
    ///
    /// The workflow copilot is what this exists for: its pane is a chat, so the
    /// second instruction has to know what the first one did. A workflow's
    /// `agent` nodes deliberately do *not* set it — each node is its own unit of
    /// work, and sharing context between them would make a graph's behaviour
    /// depend on the order its branches happened to run in.
    ///
    /// The value is scoped to the authenticated sender by the worker, so one
    /// peer cannot name another's conversation and read its context.
    ///
    /// [`route_session_class`]: crate::sessions::route_session_class
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub conversation: Option<String>,
    /// Reported on `reply` frames when the child harness surfaced token counts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage: Option<TokenUsage>,
    /// What the child harness is working on, as of this frame.
    ///
    /// Carried on `status` and `reply` frames so an orchestrator sees the
    /// worker's todo list, plan, sub-agents, and file edits rather than only the
    /// one-line detail `text` — the whole point of a master terminal is that the
    /// remote screen is legible from here. Additive and optional in both
    /// directions: a peer that predates it omits the key, and a peer that does
    /// not understand it drops it.
    ///
    /// Boxed so a frame stays small in the enums that carry it by value: the
    /// snapshot dwarfs every other field, and most frames have none.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub work: Option<Box<crate::harness_work::WorkSnapshot>>,
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
    /// Inbound-only named custom harness preset.
    pub custom_harness: Option<String>,
    /// Inbound-only advisory model hint (parallels `provider`); `None` on the
    /// responses a worker daemon emits.
    pub model: Option<String>,
    /// Inbound-only: which slice of the workflow tools the harness is served.
    pub tool_mode: Option<String>,
    /// Inbound-only: the installed workflow to run instead of treating `text` as
    /// an instruction. `None` on every response and on ordinary tasks.
    pub workflow: Option<String>,
    /// Inbound-only: the continuity group successive tasks share a session
    /// through. `None` on every response and on ordinary tasks, which stay
    /// context-free by design — see [`TaskFrame::conversation`].
    pub conversation: Option<String>,
}

/// What an agent reports it can do, merged with facts its host establishes.
///
/// Field names match the public protocol JSON (camelCase for the multi-word
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
    /// Named OpenRouter-backed harness presets this host accepts in task frames.
    ///
    /// These descriptors intentionally omit endpoint and credential details.
    /// They are selection metadata, not execution configuration.
    #[serde(
        rename = "customHarnesses",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub custom_harnesses: Vec<CustomHarnessAdvert>,
    /// Tool names the agent exposes.
    #[serde(default, deserialize_with = "de_string_array")]
    pub tools: Vec<String>,
    /// MCP server names the agent has configured.
    #[serde(rename = "mcpServers", default, deserialize_with = "de_string_array")]
    pub mcp_servers: Vec<String>,
    /// A free-text summary of the agent, when reported.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub summary: Option<String>,
    /// Best-effort, per-harness token budgets (see [`HarnessBudget`]).
    ///
    /// Additive and backward-compatible: a peer that predates the budget surface
    /// omits the key, which deserializes to an empty vector; an empty vector
    /// serializes to nothing, so old peers still parse the frame. Advisory only —
    /// budget accounting is soft and must never block delegation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub budgets: Vec<HarnessBudget>,
    /// Per-provider readiness for the installed harnesses (see [`HarnessReadiness`]).
    ///
    /// Same backward-compatibility contract as [`AgentCapabilities::budgets`]:
    /// absent/empty is tolerated in both directions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness: Vec<HarnessReadiness>,
    /// Workflows this worker has installed and can be asked to run (see
    /// [`WorkflowAdvert`]).
    ///
    /// This is how an orchestrator learns that a worker can do more than take an
    /// instruction: each entry is an id it may name in a task frame's `workflow`
    /// field. Same backward-compatibility contract as the two vectors above.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<WorkflowAdvert>,
}

/// Fleet-safe description of one named custom harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomHarnessAdvert {
    /// Stable id an orchestrator may send as `customHarness`.
    pub id: String,
    /// Operator-facing label.
    pub name: String,
    /// Coding CLI that executes the task.
    pub base_harness: HarnessProvider,
    /// OpenRouter model id.
    pub model: String,
    /// Whether this preset handles tasks that name no custom harness.
    #[serde(default)]
    pub default: bool,
}

/// One workflow a worker advertises as runnable.
///
/// Deliberately just enough for an orchestrator to choose one and explain the
/// choice — the graph itself stays on the worker, where it is authored and
/// validated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowAdvert {
    /// The id to name in a task frame's `workflow` field.
    pub id: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// What the workflow does, for an orchestrator deciding whether it fits.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// How many steps it has — a rough cost signal.
    #[serde(rename = "nodeCount", default)]
    pub node_count: usize,
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
