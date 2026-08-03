//! The `medulla.screen.v1` protocol: streaming a worker's live terminal to a
//! watching orchestrator as synchronised *state* rather than a byte stream.
//!
//! Modelled on mosh's State Synchronization Protocol. The worker already runs a
//! terminal emulator over each harness pty, so what crosses the wire is the
//! resulting screen — a grid of styled cells plus a cursor — carried as a diff
//! from the state the viewer is known to hold. Three properties follow, and they
//! are the reason this is affordable over an encrypted mailbox:
//!
//! - **Cost tracks the screen, not the output.** A harness dumping a build log
//!   costs the same as an idle one, because only the state at each sample
//!   instant is ever sent.
//! - **Loss is self-healing.** A frame that cannot be applied is not retried;
//!   the viewer asks to resynchronise and the next full frame supersedes
//!   everything missed. Dropped, duplicated and reordered frames share one
//!   recovery path, which matters because mailbox ordering is not guaranteed.
//! - **Geometry belongs to the sender.** The viewer is a passive observer: it
//!   never resizes the session and never types into it. There is no `input` and
//!   no `resize` message, so there is nothing to negotiate and nothing to fight
//!   over.
//!
//! Split by responsibility: [`types`] holds the wire model, [`diff`] is the
//! sender's cell-coalescing and row diffing, [`apply`] is the viewer's fold, and
//! [`codec`] is the version-tagged envelope this shares the DM channel with
//! three other protocols through.
//!
//! Everything here is pure and free of `vt100`, `portable-pty` and the network,
//! so it is testable against literal grids; the sampler, transport and renderer
//! that drive it live in the app crate.

pub mod apply;
pub mod codec;
pub mod diff;
pub mod types;

#[cfg(test)]
mod tests;

pub use apply::apply_frame;
pub use codec::{encode_screen_message, parse_screen_message};
pub use diff::{build_frame, changed_rows, coalesce_runs};
pub use types::{
    ApplyOutcome, Color, FrameDecision, RowUpdate, RunStyle, ScreenEnvelope, ScreenFrame,
    ScreenGrid, ScreenMessage, ScreenRun, ScreenView, ATTR_BOLD, ATTR_INVERSE, ATTR_ITALIC,
    ATTR_UNDERLINE, SCREEN_PROTO,
};
