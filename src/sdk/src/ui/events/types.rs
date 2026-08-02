//! The event data model: the [`TuiEvent`] union and its payload structs plus the
//! sequenced [`EventEnvelope`]. These are plain data types; the custom
//! serialization lives in [`super::serde_impl`] and the read-only derivations in
//! [`super::derive`].

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::harness_contract::{VerificationEvidence, WorkerContract};

/// Token accounting reported alongside an inference or task result.
///
/// The cache fields are optional because not every provider reports them and
/// not every wire carries them: absent means "not reported", which the UI
/// renders as no cache segment rather than as zero. Reading a missing number as
/// 0% would claim a cache miss that was never measured.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    /// Tokens consumed by the prompt.
    #[serde(rename = "inputTokens")]
    pub input_tokens: i64,
    /// Tokens produced by the model.
    #[serde(rename = "outputTokens")]
    pub output_tokens: i64,
    /// Prompt tokens served from the provider's cache, when reported.
    #[serde(
        default,
        rename = "cacheReadTokens",
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_read_tokens: Option<i64>,
    /// Prompt tokens written into the provider's cache, when reported.
    #[serde(
        default,
        rename = "cacheCreationTokens",
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_creation_tokens: Option<i64>,
}

impl Usage {
    /// The share of prompt tokens that came from cache, when both numbers are
    /// known and the prompt was non-empty.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let cached = self.cache_read_tokens? as f64;
        // `inputTokens` is the whole prompt, cached portion included, so the
        // ratio is against it rather than against cached + uncached.
        let total = self.input_tokens as f64;
        (total > 0.0).then(|| (cached / total).clamp(0.0, 1.0))
    }
}

/// A single tool invocation emitted by a completed inference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// The tool's name.
    pub name: String,
    /// The tool arguments as raw JSON; defaults to `null` when absent.
    #[serde(default)]
    pub args: Value,
}

/// The terminal summary of a task: status, digest text, and optional result/usage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskDigest {
    /// The task identifier this digest closes out.
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// The final task status (e.g. `done`).
    pub status: String,
    /// A short human-readable digest of the outcome.
    #[serde(default)]
    pub digest: String,
    /// An optional reference to the full result payload.
    #[serde(default, rename = "resultRef", skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<Value>,
    /// Token usage accrued by the task, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// The task's nesting depth in the cycle tree.
    #[serde(default)]
    pub depth: i64,
    /// Advisory boundaries supplied at delegation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<WorkerContract>,
    /// Verification observations produced in this task's scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Vec<VerificationEvidence>>,
}

/// One node-execution trace entry for the Trace / Overview lists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeTrace {
    /// The node that ran.
    pub node: String,
    /// Wall-clock duration in milliseconds.
    #[serde(default)]
    pub ms: i64,
    /// The tool the node invoked, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The operation the node performed, if any (used when no `tool` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
}

/// The full event union. Unknown kinds ride through in `Unknown`.
#[derive(Debug, Clone, PartialEq)]
pub enum TuiEvent {
    /// An inference call began.
    InferenceStart {
        /// Model tier selected by orchestration.
        tier: String,
        /// Operation being performed.
        op: String,
        /// Concrete model identifier, when known.
        model: Option<String>,
    },
    /// An inference call finished, with timing, usage, and any output.
    InferenceEnd {
        /// Model tier selected by orchestration.
        tier: String,
        /// Operation that completed.
        op: String,
        /// Concrete model identifier, when known.
        model: Option<String>,
        /// Wall-clock inference duration in milliseconds.
        duration_ms: i64,
        /// Token accounting reported by the provider.
        usage: Option<Usage>,
        /// Final assistant content, when produced.
        content: Option<String>,
        /// Final reasoning content, when exposed.
        reasoning: Option<String>,
        /// Tool calls requested by the model.
        tool_calls: Option<Vec<ToolCall>>,
    },
    /// A streamed tool call started at `index`.
    ToolCallStart {
        /// Position of the call in the model response.
        index: i64,
        /// Requested tool name.
        name: String,
    },
    /// A chunk of streamed tool-call arguments.
    ToolCallDelta {
        /// Position of the call in the model response.
        index: i64,
        /// Newly emitted argument JSON text.
        args_delta: String,
    },
    /// A chunk of streamed assistant text.
    AssistantDelta {
        /// Newly emitted assistant text.
        delta: String,
    },
    /// A chunk of streamed reasoning text.
    ReasoningDelta {
        /// Newly emitted reasoning text.
        delta: String,
    },
    /// A task began.
    TaskStart {
        /// Stable task identifier.
        task_id: String,
        /// Instruction delegated to the task.
        instruction: String,
        /// Task depth in the delegation tree.
        depth: i64,
        /// Agent selected for the task.
        agent_id: Option<String>,
        /// Advisory execution and verification boundaries.
        contract: Option<WorkerContract>,
    },
    /// A task emitted an event (text, status, etc.).
    TaskEvent {
        /// Stable task identifier.
        task_id: String,
        /// Open-vocabulary task event kind.
        event_kind: String,
        /// Human-readable event content.
        content: String,
        /// Harness that emitted the event.
        harness: Option<String>,
    },
    /// A task needs operator attention (e.g. a confirmation).
    TaskAttention {
        /// Stable task identifier.
        task_id: String,
        /// Why operator input is required.
        reason: String,
        /// Human-readable prompt or context.
        content: String,
        /// Question identifier used when answering.
        question_id: Option<String>,
    },
    /// A task finished; carries its [`TaskDigest`].
    TaskComplete {
        /// Terminal task summary.
        digest: TaskDigest,
    },
    /// A node-execution trace entry.
    Trace {
        /// Trace entry emitted by the node.
        entry: NodeTrace,
    },
    /// An error surfaced by a source component.
    Error {
        /// Component that reported the error.
        source: String,
        /// Human-readable error detail.
        message: String,
    },
    /// A cycle began.
    CycleStart {
        /// Stable cycle identifier.
        cycle_id: String,
    },
    /// A cycle finished, with its pass count and duration.
    CycleEnd {
        /// Stable cycle identifier.
        cycle_id: String,
        /// Number of orchestration passes consumed.
        pass_count: i64,
        /// Wall-clock cycle duration in milliseconds.
        duration_ms: i64,
    },
    /// An agent's availability changed.
    AgentStatus {
        /// Stable roster agent identifier.
        agent_id: String,
        /// Open-vocabulary availability state.
        availability: String,
        /// Optional provider-specific status detail.
        detail: Option<String>,
    },
    /// A session emitted an event on an agent.
    SessionEvent {
        /// Agent that owns the session.
        agent_id: String,
        /// Stable session identifier.
        session_id: String,
        /// Open-vocabulary session event kind.
        event_kind: String,
        /// Human-readable event content.
        content: String,
    },
    /// A peer session's state changed.
    PeerSession {
        /// Peer agent that owns the session.
        agent_id: String,
        /// Stable session identifier.
        session_id: String,
        /// Open-vocabulary session state.
        state: String,
        /// Harness serving the session.
        harness: Option<String>,
    },
    /// A user chat turn.
    User {
        /// User-authored message text.
        body: String,
    },
    /// An assistant chat turn.
    Assistant {
        /// Assistant-authored message text.
        body: String,
    },
    /// A host effect (opaque JSON).
    Effect {
        /// Opaque effect payload retained for the host.
        effect: Value,
    },
    /// An unrecognized event kind, preserved verbatim for forward compatibility.
    Unknown {
        /// Unrecognized wire event kind.
        kind: String,
        /// Remaining event fields preserved verbatim.
        data: Map<String, Value>,
    },
}

/// A sequenced event with its wall-clock timestamp (epoch ms).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope {
    /// Monotonic sequence number within the stream.
    pub seq: u64,
    /// Wall-clock timestamp in epoch milliseconds.
    pub at: i64,
    /// The wrapped event.
    pub event: TuiEvent,
}
