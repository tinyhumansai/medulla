//! The transparent harness wrapper behind `medulla codex` / `medulla claude` /
//! `medulla opencode`.
//!
//! The wrapper launches the real coding-agent CLI in the user's terminal exactly
//! as if it were run directly (inherited stdio — no PTY re-implementation), while
//! bridging the session to tiny.place underneath: it tails the harness's own
//! JSONL transcript, normalizes each record into a typed
//! [`SessionEnvelopeV2`](crate::tinyplace::SessionEnvelopeV2) event, and
//! forwards the stream as encrypted Signal DMs to the configured owner. When
//! inbound input is enabled it also polls the mailbox for owner→session control
//! frames and types their text into the child.
//!
//! This is the single-terminal `--raw` mode of the TypeScript `tinyplace codex`
//! command, ported to Rust. It reuses the existing medulla pieces rather than
//! duplicating them: transcript discovery ([`crate::session_history`]), record →
//! event mapping ([`crate::daemon::mappers`]), the derived status machine
//! ([`crate::tinyplace::status`]), encrypted transport
//! ([`crate::daemon::transport::SignalTransport`]), and identity/config bootstrap
//! ([`crate::tinyplace`]).
//!
//! ## Scope cuts (deliberately not built here)
//!
//! These parts of the TypeScript wrapper are out of scope for this single-terminal
//! slice and are intentionally omitted:
//! - the tinyplace TUI chrome mode and the `--agent` plugin mode;
//! - the **machine bus** multi-terminal coordination (wallet lock, session spool,
//!   inbox routing) — one terminal, one session, direct id matching instead;
//! - the **opencode SSE server** bridge — opencode therefore runs as a passthrough
//!   with input injection but **no transcript tailing** (its session log is not a
//!   flat JSONL the mappers read);
//! - the terminal-envelope writer (raw keystroke/output capture);
//! - `node-pty`: stdio is inherited. For a pristine full-screen TUI, run without
//!   inbound input (or `--no-bridge`) so stdin stays attached to the terminal;
//!   enabling input injection pipes stdin (a best-effort byte pump), which a
//!   full-screen TUI may not drive perfectly.
//!
//! ## Module layout
//!
//! Split by responsibility: [`types`] holds the session data model
//! ([`WrapperConfig`]), [`args`] parses the wrapper's own flags, [`bridge`] owns
//! the tiny.place [`Bridge`](bridge::Bridge) and its transcript/inbox I/O, and
//! [`run`] drives the child process and select loop. The `control`, `tail`, and
//! `envelope` submodules provide frame targeting, transcript tailing, and
//! envelope construction. All public items are re-exported here so callers use
//! `medulla::wrapper::*`.

pub mod control;
pub mod envelope;
pub mod tail;

mod args;
mod bridge;
mod run;
mod types;

#[cfg(test)]
mod tests;

pub use args::parse_wrapper_args;
pub use run::{run_wrapper, run_wrapper_with};
pub use types::WrapperConfig;
