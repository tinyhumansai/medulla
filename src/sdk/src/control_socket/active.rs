//! The control plane this process is serving, if it bound one.
//!
//! A process-wide slot rather than a value threaded through every call, because
//! the thing it describes *is* process-wide: one socket, bound once, outside any
//! session. The alternative was passing a registry handle from the TUI's startup
//! down through the hub, the daemon, and the provider layer to reach the one
//! function that mints a grant — five signatures widened to carry something none
//! of them have an opinion about.
//!
//! Set once and never replaced: a second `install` is refused rather than
//! overwriting, so a stray caller cannot redirect grant minting at a socket the
//! operator did not bind.

use std::path::PathBuf;
use std::sync::OnceLock;

use super::grants::GrantRegistry;

/// Where spawned harnesses reach this process, and what they may do there.
#[derive(Clone)]
pub struct ActiveControlPlane {
    /// The bound socket path, handed to each spawned harness.
    pub socket: PathBuf,
    /// Where this process's grants are minted and redeemed.
    pub grants: GrantRegistry,
    /// How deep a dispatch tree may go before dispatching is withheld.
    pub max_depth: u8,
    /// The most concurrent dispatches one grant may hold.
    pub max_in_flight: usize,
}

static ACTIVE: OnceLock<ActiveControlPlane> = OnceLock::new();

/// Record the control plane this process bound.
///
/// Returns `false` if one was already installed, which is not an error worth
/// failing a startup over — it means something else already bound, and that
/// binding is the one to keep.
pub fn install(plane: ActiveControlPlane) -> bool {
    ACTIVE.set(plane).is_ok()
}

/// The control plane this process bound, if any.
///
/// `None` on every process that never bound one — a remote worker's daemon, a
/// `medulla workflow` invocation, a build with fleet tools switched off — and
/// that is precisely what leaves a spawned harness with no fleet grant.
pub fn active() -> Option<&'static ActiveControlPlane> {
    ACTIVE.get()
}
