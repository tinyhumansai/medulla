//! The handoff brief and the control state it accompanies.

use serde::{Deserialize, Serialize};

/// Who holds a harness, as the orchestrator is told.
///
/// The SDK-side spelling of the TUI's `HarnessControl`. Deliberately a second
/// type rather than `serde` on the first: that enum's contract is that it is
/// process-local and never serialized, and it is the single gate on dispatch.
/// Deriving `Serialize` onto it would quietly make it a wire type and put the
/// dispatch gate one careless `#[serde(default)]` away from being decided by a
/// remote peer.
///
/// The wire spelling is `operator`, not `user`: medulla-v1 reasons about
/// operators and orchestrators, and one word for one concept across the two
/// repos is worth more than matching the local enum's variant name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HandoffControl {
    /// The orchestrator may dispatch into this harness.
    #[default]
    Orchestrator,
    /// A person is working in it. Nothing may be dispatched there.
    Operator,
}

impl HandoffControl {
    /// The wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            HandoffControl::Orchestrator => "orchestrator",
            HandoffControl::Operator => "operator",
        }
    }

    /// Whether a person holds it.
    pub fn is_operator(self) -> bool {
        matches!(self, HandoffControl::Operator)
    }
}

/// What an operator leaves behind when they hand a running harness back.
///
/// A *description of work in progress*, not a resumable session. The
/// orchestrator never reopens the operator's session — it reads this and places
/// a manager on the same harness to continue the thread. That distinction is
/// why `transcript` is a bounded excerpt rather than a handle: there is nothing
/// on the far end to resume.
///
/// `note` and `transcript` are operator and terminal text, so anything
/// downstream must treat them as untrusted data describing work, never as
/// instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessHandoff {
    /// Identifies this handback *event*, not the session.
    ///
    /// A second handback of the same harness mints a new id. Reusing one means
    /// the orchestrator, having already picked that id up, silently ignores the
    /// new work — which looks exactly like the handoff never happening.
    pub id: String,
    /// Epoch ms the operator handed it back.
    pub at: i64,
    /// The local PTY session id (`w_…`). Rendered for a human matching a manager
    /// back to the pane they were in; never resolved by anything downstream.
    pub session_id: String,
    /// The harness's own session/transcript id, when one is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_session_id: Option<String>,
    /// Which CLI was running (`claude`, `codex`, …).
    pub provider: String,
    /// The working directory the session was in. Must match a workspace the same
    /// harness exposes, or the pickup cannot be placed.
    pub workspace_path: String,
    /// Git branch checked out there at handoff, verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The repository name, when the workspace is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// What the operator wants continued, in their words. Untrusted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The tail of the pane — what was on screen when they handed it over.
    /// Bounded (see `normalize`). Untrusted.
    pub transcript: String,
    /// Whether anything was dropped to fit the bounds.
    ///
    /// On the wire rather than inferred, so a reader knows it is looking at an
    /// excerpt and not at the whole of a very short session.
    pub transcript_truncated: bool,
}
