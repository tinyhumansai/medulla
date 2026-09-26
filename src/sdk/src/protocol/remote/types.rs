//! Wire types for `medulla.remote.v1`.
//!
//! Every type here is plain serde with no dependency on the pty layer, for the
//! same reason [`screen`](super::super::screen) has none: the app crate owns
//! `vt100` and `portable-pty`, and a protocol that reached into them could not be
//! tested without starting a child process.

use serde::{Deserialize, Serialize};

/// Wire version tag carried by every [`RemoteEnvelope`].
pub const REMOTE_PROTO: &str = "medulla.remote.v1";

/// Where a remote session's child is in its life.
///
/// A deliberately smaller mirror of the app crate's `PtyState`: the client draws
/// a row, it does not manage the child, so "running, gone, or never started" is
/// the whole of what has to cross.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RemoteSessionState {
    /// The child is running.
    Running,
    /// The child exited; the daemon keeps its last screen so it can still be read.
    Exited {
        /// The child's exit status, when it reported one.
        code: Option<i32>,
    },
    /// The child could not be started, or its pty died.
    Failed,
}

/// What a remote harness is waiting on the operator for.
///
/// Carried because it is the one thing no other field on the row can express: a
/// harness stopped on a permission prompt is running, not busy, and producing no
/// output, so every other field reads exactly as it does for one thinking hard.
/// Without this the operator watching another pane never learns it stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAttention {
    /// Short description of what is being asked, for the row's cue.
    pub summary: String,
    /// Why it is waiting, as the sender's own cue name.
    ///
    /// Carried rather than inferred from [`summary`](Self::summary): the rail
    /// styles a permission prompt differently from a rung bell, and a viewer
    /// guessing from prose would get it wrong in exactly the cases that matter.
    /// An unrecognized value means "asking, reason unknown" — which is still a
    /// harness waiting on somebody, and must not be dropped.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

/// One session on a remote host, as the client's rail draws it.
///
/// Deliberately a subset of the app crate's `SessionRow`. The fields left out —
/// checkout tracking, launch commit anchoring, MCP grant keys — all exist to
/// serve views that act on the *local* filesystem, and a client cannot act on
/// the far side's. Adding them later is additive; sending them now would be
/// inventing a contract nothing reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionRow {
    /// The daemon's own session id (`w_…`), unique on that host.
    ///
    /// The client prefixes it with the host id to make it unique here; see the
    /// client's session registry. It is *not* prefixed on the wire, because the
    /// daemon has never heard of the name the client files it under.
    pub id: String,
    /// The list label.
    pub label: String,
    /// Which harness is running, as its wire name.
    pub provider: String,
    /// The custom preset it was launched from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Where the child is in its life.
    #[serde(flatten)]
    pub state: RemoteSessionState,
    /// The working directory the child runs in, on the remote host.
    pub cwd: String,
    /// The name the operator gave it when they started it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Thread name the harness advertised through its terminal title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_name: Option<String>,
    /// Epoch ms when the session started.
    pub started_at: i64,
    /// Epoch ms of the last output byte.
    pub last_output_at: i64,
    /// Whether a turn is running right now.
    pub busy: bool,
    /// Whether the harness is mid-turn per its own screen.
    pub working: bool,
    /// What it is waiting on the operator for, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<RemoteAttention>,
    /// Whether the session's emulator has bracketed paste enabled.
    ///
    /// Sent because the *client* encodes keystrokes ([`ScreenMessage::Input`] is
    /// bytes, not key events), so it has to know the modes the far-side emulator
    /// is in. Without this a paste into a remote session would be bracketed
    /// wrongly, which harnesses notice.
    ///
    /// [`ScreenMessage::Input`]: super::super::screen::ScreenMessage::Input
    pub bracketed_paste: bool,
    /// Why it failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// One thing a remote host can start a session on.
///
/// The remote equivalent of the picker's harness choice. It has to come from the
/// daemon rather than be assumed, because what is installed over there is not
/// what is installed here — and offering a `claude` row for a host that has no
/// `claude` breaks the picker's standing contract that it never shows a row that
/// cannot start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHarnessChoice {
    /// Stable id, matching the client's own harness-choice ids.
    pub id: String,
    /// Wire name of the underlying provider (`claude`, `codex`, `shell`, …).
    pub provider: String,
    /// The custom preset this row stands for, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// What to show in the picker.
    pub display_name: String,
}

/// What a remote host can do, answered once when a client says hello.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCapabilities {
    /// The daemon's medulla version, so a client can say plainly when a mismatch
    /// is the reason something is missing.
    pub version: String,
    /// Everything a session could be started on there, in offer order.
    pub harnesses: Vec<RemoteHarnessChoice>,
    /// The directory a session starts in when the client names none.
    pub workspace: String,
    /// Other directories worth offering on the picker's workspace step.
    ///
    /// A remote directory cannot be completed from the client — there is no
    /// filesystem to walk — so this list is the whole of what the picker can
    /// offer beyond what the operator types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<String>,
    /// The host's own name for itself, for the rail when config named none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_name: String,
}

/// Everything that crosses `medulla.remote.v1`, in both directions.
///
/// Session *control* only. The screen itself, and the keystrokes going back to
/// it, travel on [`medulla.screen.v1`](super::super::screen) — which already
/// models a synchronised screen and already has somewhere for input to go, and
/// which a remote session shares verbatim with a locally-watched one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteMessage {
    /// Client → daemon: I am here; what can you do?
    Hello {
        /// The client's medulla version, for the same mismatch reporting.
        client_version: String,
    },
    /// Daemon → client: this is what I can do.
    Capabilities(RemoteCapabilities),
    /// Client → daemon: start a session.
    Open {
        /// Correlates the answer, since several opens may be in flight.
        request_id: String,
        /// What to start.
        harness: RemoteHarnessChoice,
        /// Where to start it. Empty means the host's default workspace.
        workspace: String,
        /// The pane's width, so the pty starts at the right size rather than
        /// starting at a default and reflowing on the first resize.
        cols: u16,
        /// The pane's height.
        rows: u16,
        /// The name the operator gave it, if they gave one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Daemon → client: it started, and this is its id.
    Opened {
        /// Echoes the request.
        request_id: String,
        /// The new session's id on that host.
        session_id: String,
    },
    /// Daemon → client: it did not start, and this is why.
    OpenFailed {
        /// Echoes the request.
        request_id: String,
        /// Operator-facing reason, passed through from the launch.
        reason: String,
    },
    /// Client → daemon: end a session.
    Close {
        /// The session to close.
        session_id: String,
    },
    /// Daemon → client: the whole session list, latest wins.
    ///
    /// Sent whole rather than as deltas because it is small, changes rarely, and
    /// a lost delta would leave the rail permanently wrong — the same reasoning
    /// that makes the screen channel latest-wins.
    Sessions {
        /// Every session on the host, in the daemon's own order.
        rows: Vec<RemoteSessionRow>,
    },
}

/// A remote message with its version tag, as it appears on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteEnvelope {
    /// Wire version tag ([`REMOTE_PROTO`]).
    pub remote_version: String,
    /// The message itself.
    #[serde(flatten)]
    pub message: RemoteMessage,
}
