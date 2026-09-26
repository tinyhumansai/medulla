//! Private state retained while folding an ACP session stream.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;

use super::super::types::OnEvent;
use super::super::types::OnWorkspaceContext;
use crate::daemon::mappers::PendingPullRequestCall;
use crate::sessions::WorkspaceContext;

/// Provider metadata accumulated across ACP's initial call and later patches.
#[derive(Default)]
pub(super) struct AcpToolCall {
    /// Human-facing title supplied by the provider.
    pub(super) title: String,
    /// Provider tool kind used to classify the call.
    pub(super) kind: String,
    /// Most recent structured input for the call.
    pub(super) input: Value,
}

/// One ACP tool that reached a terminal state.
pub(super) struct CompletedToolCall {
    /// ACP's provider tool kind, used by lifecycle-hook matchers.
    pub(super) tool_name: String,
    /// The final structured input observed for the call.
    pub(super) input: Value,
}

/// Mutable transcript and metadata accumulated from an ACP session stream.
pub(in crate::daemon::providers) struct FoldState {
    /// Assistant response text accumulated across message chunks.
    pub(super) text: String,
    /// Current bounded reasoning snapshot accumulated across thought chunks.
    pub(super) thought: String,
    /// Number of semantic events emitted so far.
    pub(super) events: usize,
    /// Optional observer receiving each folded semantic event.
    pub(super) on_event: Option<OnEvent>,
    /// Time of the most recent provider update, used for idle detection.
    pub(super) last_activity: Instant,
    /// Tool metadata retained until each call settles.
    pub(super) tool_calls: HashMap<String, AcpToolCall>,
    /// PR commands waiting for their ACP tool result.
    pub(super) pull_request_calls: HashMap<String, PendingPullRequestCall>,
    /// Repository identity retained across ACP turns.
    pub(super) workspace_context: WorkspaceContext,
    /// Whether `GH_REPO` makes checkout-local PR output non-authoritative.
    pub(super) gh_repo_is_set: bool,
    /// Observer persisting changed workspace state into the session binding.
    pub(super) on_workspace_context: Option<OnWorkspaceContext>,
    /// The turn-fatal error the agent reported, if it reported one.
    ///
    /// An ACP agent may answer `session/prompt` with `stopReason: end_turn`
    /// after a turn that never produced an answer — `codex-acp` does exactly
    /// that for an upstream HTTP failure, streaming the provider's error text
    /// as an ordinary assistant message. The stop reason therefore cannot
    /// decide whether the turn succeeded; this can.
    pub(super) error: Option<String>,
    /// The most recent error the agent said it would retry.
    ///
    /// `codex-acp` announces each attempt as a `willRetry` error and then
    /// gives up with a bare `systemError` thread status carrying no message of
    /// its own, so the last retried error is the only description of what
    /// actually went wrong.
    pub(super) last_retried_error: Option<String>,
}
