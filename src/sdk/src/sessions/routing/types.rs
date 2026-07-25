//! Data types for the `routing` module.
#[allow(unused_imports)]
use super::*;
/// What provoked a session. The routing input that is *not* operator policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stimulus {
    /// A `medulla-tinyplace/1` `task` frame — a discrete unit of delegated work.
    Task,
    /// A conversational plain-text DM from a peer.
    PlainText,
    /// The operator opened a session from the TUI.
    Operator,
}
/// How a provider's child process is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// One process per turn: spawn, feed the prompt as argv, read to EOF.
    ///
    /// Continuity, when the provider supports it, comes from resuming a captured
    /// session id rather than from keeping the process alive.
    OneShot,
    /// One long-lived process fed newline-delimited JSON turns over stdin.
    Interactive,
}
