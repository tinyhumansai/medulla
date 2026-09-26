//! Sessions that run on another machine.
//!
//! Two halves that never run in the same process:
//!
//! - [`serve`] is the daemon side — it owns the real ptys and relays their
//!   screens. Reached by `medulla daemon --direct`.
//! - [`client`] is the other end — it bootstraps a host over SSH, dials it, and
//!   folds what comes back into a screen the ordinary renderer can draw.

pub mod client;
pub mod serve;
