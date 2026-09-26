//! The session-binding registry: which harness session id a conversation is
//! bound to, and the per-key serialization that keeps turns from interleaving.
//!
//! [`behavior`] owns the binding lifecycle and serialization rules; [`types`]
//! holds the public plans, registry handle, and retained workspace state.

mod behavior;
mod types;

pub use behavior::DEFAULT_MAX_BINDINGS;
pub use types::{SessionIdentity, SessionRegistry, TurnPlan, WorkspaceContext};
