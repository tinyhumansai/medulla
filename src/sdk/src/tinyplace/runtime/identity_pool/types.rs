//! Data model for identity-slot acquisition: the lock guard that makes a slot
//! exclusive, and the bundle handed back to the daemon once one is held.

use std::fs::File;
use std::path::{Path, PathBuf};

use ::tinyplace::LocalSigner;

use crate::tinyplace::config::TinyplaceFileConfig;

/// An exclusive hold on one identity slot.
///
/// The lock is an advisory whole-file lock (`flock` on unix, `LockFileEx` on
/// Windows) taken on `<slot>/tinyplace/daemon.lock`. Two properties matter:
///
/// - it is held for as long as this value lives, so the daemon must keep the
///   guard alive for its whole process lifetime — dropping it early lets a
///   second daemon take the same tiny.place address; and
/// - the OS releases it when the holding process dies, however it dies, so a
///   crashed or `SIGKILL`ed daemon leaves its slot immediately reclaimable.
///   That is the whole reason for a lock rather than a pidfile.
pub struct IdentityLock {
    file: File,
    path: PathBuf,
}

impl IdentityLock {
    /// Wrap an already-locked file. Constructed only by
    /// [`super::try_lock_slot`], which is what performs the lock.
    pub(super) fn new(file: File, path: PathBuf) -> Self {
        IdentityLock { file, path }
    }

    /// The lock file this guard holds.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IdentityLock {
    fn drop(&mut self) {
        // Closing the file would release the lock on its own; unlocking first is
        // explicit about the intent and makes a released slot observable in
        // tests without relying on close ordering. Fully qualified because
        // `std::fs::File` grew an inherent `unlock` that would otherwise shadow
        // the `fs2` trait method.
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

impl std::fmt::Debug for IdentityLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityLock")
            .field("path", &self.path)
            .finish()
    }
}

/// A tiny.place identity the daemon now holds exclusively.
///
/// Everything downstream (`SignalTransport`, the endpoint resolver, the agent
/// id) is derived from these fields, so the caller never has to re-resolve a
/// path and risk disagreeing with the slot that was actually locked.
pub struct AcquiredIdentity {
    /// The signer for this slot's key — the identity's address.
    pub signer: LocalSigner,
    /// The slot's config file, with `secret_key` reflecting the key in use.
    pub config: TinyplaceFileConfig,
    /// Absolute path of the config file that was loaded or minted.
    pub config_path: PathBuf,
    /// The directory holding the config and the Signal session store.
    pub identity_dir: PathBuf,
    /// 1-based slot number: 1 is the pre-existing identity, 2+ are the
    /// `workers/<N>` fan-out slots.
    pub slot: usize,
    /// The hold on the slot. Keep it alive for the process lifetime.
    pub lock: IdentityLock,
}

impl std::fmt::Debug for AcquiredIdentity {
    /// Hand-written, and deliberately partial: `config` holds the secret seed,
    /// so it is named but never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcquiredIdentity")
            .field("slot", &self.slot)
            .field("config_path", &self.config_path)
            .field("identity_dir", &self.identity_dir)
            .finish_non_exhaustive()
    }
}
