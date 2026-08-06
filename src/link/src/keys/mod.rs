//! Node identity and key material for the host link (protocol §7.3).
//!
//! One endpoint owns one identity, persisted at `<home>/link/node.json` with
//! mode `0600`, holding the node id, the role, the pair key, the forwarder key,
//! the forwarder endpoint and the sequence reservation of §3.1.
//!
//! Acquisition mirrors the host-link identity bootstrap
//! (`tinyplace::runtime::acquire_identity`): take an exclusive advisory lock on
//! the identity directory, *then* load or mint. The lock matters more here than
//! it does there. Two processes sharing a link identity would draw sequences
//! from two independent counters under one AEAD key, which reuses nonces — so a
//! busy identity is always an error and never a fan-out to a second slot. Unlike
//! a link address, a node id is not interchangeable: the peer is talking
//! to *this* node.
//!
//! Nothing in this module performs enrollment. The caller supplies the state to
//! create from ([`acquire_or_create`]); Phase 1 has no HTTP.

mod pair_key;
mod seq;
mod store;
mod types;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

pub use pair_key::{PairKey, PairKeyError};
pub use seq::{MemorySeq, ReservedSeq, SeqSource, RESERVATION_BLOCK};
pub use store::{node_path, read_node_state, LOCK_FILE, NODE_FILE};
pub use types::{
    EnrolledPeer, ForwarderKey, KeyError, KeyResult, NodeId, NodeLock, NodeState, Role,
};

/// The identity directory under a medulla home: `<home>/link`.
pub fn link_dir(home: &Path) -> PathBuf {
    home.join("link")
}

/// An identity this process now holds exclusively.
///
/// **Keep [`AcquiredNode::lock`] alive for the process lifetime.** Dropping it
/// lets a second process open the same node and issue sequences from its own
/// reservation, which is nonce reuse.
#[derive(Debug)]
pub struct AcquiredNode {
    /// The loaded (or freshly created) identity.
    pub state: NodeState,
    /// Absolute path of the state file.
    pub path: PathBuf,
    /// The sequence source, already holding a persisted reservation.
    pub seq: ReservedSeq,
    /// The hold on the identity directory.
    pub lock: NodeLock,
}

/// Open the identity in `dir`, failing when it does not exist.
///
/// # Errors
///
/// [`KeyError::NotEnrolled`] when there is no `node.json`, [`KeyError::Busy`]
/// when another process holds the identity, [`KeyError::Malformed`] when the
/// file does not parse — a malformed file is reported, never replaced, because
/// it holds the only copy of a pair key that has no recovery path.
pub fn acquire(dir: &Path) -> KeyResult<AcquiredNode> {
    let lock = store::lock_node_dir(dir)?;
    let path = node_path(dir);
    let state = store::read_node_state(&path)?;
    finish(path, state, lock)
}

/// Open the identity in `dir`, creating it from `seed` when absent.
///
/// `seed` is only called when there is no identity yet, and only while the lock
/// is held, so two racing processes cannot both mint one.
pub fn acquire_or_create(dir: &Path, seed: impl FnOnce() -> NodeState) -> KeyResult<AcquiredNode> {
    let lock = store::lock_node_dir(dir)?;
    let path = node_path(dir);
    let state = match store::read_node_state(&path) {
        Ok(state) => state,
        Err(KeyError::NotEnrolled(_)) => {
            let state = seed();
            store::write_node_state(&path, &state)?;
            state
        }
        Err(error) => return Err(error),
    };
    finish(path, state, lock)
}

/// Attach a persisted sequence reservation to a loaded identity.
fn finish(path: PathBuf, state: NodeState, lock: NodeLock) -> KeyResult<AcquiredNode> {
    let seq = ReservedSeq::open(&path, state.clone())?;
    Ok(AcquiredNode {
        state,
        path,
        seq,
        lock,
    })
}
