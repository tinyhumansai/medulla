//! The control plane a spawned harness reaches Medulla's live fleet through.
//!
//! Medulla drives Claude Code and Codex over ACP as a *client*, so the only way
//! it can hand a harness a tool is to offer it an MCP server — and that server
//! runs as a separate subprocess spawned by the ACP agent, not by Medulla. A
//! fresh process has no hub, no roster, and no `TaskRunner`. This module is the
//! bridge back: the orchestrator binds a unix socket, and the MCP shim proxies
//! its fleet tools over it.
//!
//! # Why a grant rather than file permissions
//!
//! The socket is private to the OS user, but that alone would make every process
//! that user runs equally entitled — which is exactly what this surface must not
//! be. Instead Medulla [mints a grant](grants::GrantRegistry::mint) immediately
//! before spawning a harness and hands the token to that one child in its
//! environment.
//!
//! The token *is* the authority. Depth in the dispatch tree, tool families, and
//! the concurrency ceiling are all recorded server-side and looked up by token,
//! so a model that rewrites its own environment or lies in a request changes
//! nothing — it can only present a token that means less. That is the difference
//! between a depth cap that holds and one a single confused turn talks past.
//!
//! Filesystem permissions remain as defence in depth: a `0700` parent directory
//! and a `0600` socket, checked in [`path`].
//!
//! # Layout
//!
//! * [`types`] — wire types and the [`FleetOps`] seam the handler is written to.
//! * [`path`] — where the socket lives, and whether it can be bound.
//! * [`grants`] — minting and redeeming capabilities.
//! * [`server`] — the listener, the pure request handler, and the task registry.
//! * [`client`] — what the MCP shim uses to reach all of the above.
//!
//! This module depends only on [`crate::hub`], never on the `workflows` feature,
//! so a build without workflows still has a working fleet plane.

pub mod active;
pub mod grants;
pub mod path;
pub mod types;

#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod server;

#[cfg(test)]
mod tests;

pub use active::{active, install, ActiveControlPlane};
pub use grants::{Grant, GrantRegistry};
pub use path::{control_socket_path, ControlSocketError, CONTROL_SOCKET_ENV};
pub use types::{
    depth_from_env, grant_from_env, ControlError, ControlFailure, ErrorKind, FleetOps, FleetWorker,
    Hello, ToolFamilies, FLEET_DEPTH_ENV, MCP_GRANT_ENV, MCP_SOCKET_ENV, PROTOCOL_VERSION,
};

#[cfg(unix)]
pub use client::ControlClient;
#[cfg(unix)]
pub use server::{
    handle_control, ControlServer, FleetDefaults, HubFleetOps, HubSlot, SessionState, TaskEntry,
    TaskRegistry, TaskState,
};
