//! Collision-free identity acquisition for the daemon.
//!
//! # Why this exists
//!
//! A worker's tiny.place address is derived **entirely** from the 32-byte seed in
//! `<medulla_home>/tinyplace/config.json` — not from its PID, its port, or its
//! `--workspace`. [`load_or_create_identity`] loads that seed unconditionally,
//! with no in-use check, so two daemons started against the same home both bind
//! the *same* address. They then both drain the one shared inbox (each message
//! goes to whichever polled first: duplication for the sender, silent loss for
//! the other worker) and both advance the same Signal double-ratchet, which
//! corrupts it for every peer.
//!
//! [`acquire_identity`] closes that hole: at startup the daemon takes an
//! exclusive OS lock on an identity slot and keeps it for its whole life, so a
//! second daemon on the same machine provably cannot land on the same address.
//!
//! # The pool (launch-order slots)
//!
//! There is no flag and no user-visible `MEDULLA_HOME`; the launcher just spawns
//! `medulla daemon` and the daemon sorts itself out.
//!
//! ```text
//! slot 1 = <medulla_home>/tinyplace/config.json          the existing identity
//! slot N = <medulla_home>/workers/<N>/tinyplace/config.json   for N = 2, 3, …
//! ```
//!
//! Slot 1 is the identity that is already on disk, so a machine running one
//! worker keeps the address it has always had — this change is invisible to it.
//! Only the second concurrent daemon fans out, reusing a free slot's key if that
//! slot has been used before, and minting one if it is new.
//!
//! The accepted trade-off: a worker's address is "whichever slot was free when it
//! launched", not something tied to its workspace or provider. A *slot's* key is
//! stable once minted (slot 2 always has the same address), but which worker
//! lands on it depends on start order. Collision-safety is the goal; stable
//! per-workspace identity is not, and would need a different scheme.
//!
//! # Explicit configuration is never redirected
//!
//! When the operator has pinned an identity — `MEDULLA_HOME`, `TINYPLACE_CONFIG`,
//! or `TINYPLACE_SECRET_KEY` — a busy slot is a **hard error**. Silently moving
//! such a daemon to `workers/2` would give it an address nobody asked for, and
//! the peers holding the pinned one would simply never be answered. `TINYPLACE_SECRET_KEY`
//! counts as pinning because it overrides every slot's on-disk key: fanning out
//! under it would hand slot 2 the same address as slot 1, which is the exact bug
//! this module exists to prevent.

mod types;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use super::super::config::config_path;
use super::identity::load_or_create_identity;
use super::types::{RuntimeError, RuntimeResult};

pub use types::{AcquiredIdentity, IdentityLock};

/// The identity file inside a slot's directory.
pub const IDENTITY_FILE: &str = "config.json";

/// How many slots to try before giving up. Far above any plausible number of
/// daemons on one machine; it exists so a pathological state (every slot locked
/// by a leaked process) fails with a message instead of scanning forever.
const MAX_SLOTS: usize = 64;

/// The lock file inside a slot's identity directory.
const LOCK_FILE: &str = "daemon.lock";

/// Acquire an exclusive tiny.place identity for this process.
///
/// `home_dir` is the fallback home base, used only when `env` carries no
/// `HOME`/`USERPROFILE` — the same seam [`config_path`] takes, so resolution
/// stays pure and testable.
///
/// Returns the signer, the config it came from, the slot's directory, and the
/// lock guard. **The caller must keep [`AcquiredIdentity::lock`] alive for the
/// process lifetime**; dropping it releases the slot while the daemon is still
/// serving on that address.
///
/// # Errors
///
/// - The identity is pinned by `MEDULLA_HOME` / `TINYPLACE_CONFIG` /
///   `TINYPLACE_SECRET_KEY` and is already held by another process. This is
///   deliberately loud: it never falls back to a different address.
/// - Every slot up to [`MAX_SLOTS`] is held.
/// - The lock file or config cannot be created, or the configured key is not a
///   valid 32-byte hex seed.
pub fn acquire_identity(
    env: &HashMap<String, String>,
    home_dir: &Path,
) -> RuntimeResult<AcquiredIdentity> {
    let slot_one = config_path(env, home_dir);

    if pins_identity(env) {
        return acquire_identity_at(&slot_one, env);
    }

    // The pool base is the directory holding slot 1's `tinyplace/` — i.e. the
    // medulla home — derived from the resolved path rather than re-resolved, so
    // the fan-out can never disagree with the slot it is fanning out from.
    let base = slot_one
        .parent()
        .and_then(Path::parent)
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::home::medulla_home(env));

    for slot in 1..=MAX_SLOTS {
        let path = if slot == 1 {
            slot_one.clone()
        } else {
            base.join("workers")
                .join(slot.to_string())
                .join("tinyplace")
                .join(IDENTITY_FILE)
        };
        if let Some(lock) = try_lock_slot(&path)? {
            // A `finish` error (e.g. a slot with a malformed key) propagates out
            // rather than advancing to the next slot: only a *busy* slot — the
            // `None` above — means "try another". A slot we hold the lock on but
            // cannot load from is a real fault to report, not a collision to work
            // around. Covered by `a_bad_planted_key_is_reported_not_replaced`.
            return finish(path, slot, lock, env);
        }
    }

    Err(RuntimeError::Invalid(format!(
        "no free tiny.place identity slot under {} (tried {MAX_SLOTS})",
        base.display()
    )))
}

/// Take one *named* identity exclusively, or fail — never a different one.
///
/// This is the fail-loud half of [`acquire_identity`], exposed for the callers
/// that resolve their own identity directory (the worker TUI reads it from the
/// `[tinyplace]` config section). Silently redirecting a caller that named its
/// identity would give it an address nobody asked for, while the peers holding
/// the named one went unanswered.
///
/// The returned slot is always 1: there is no pool here, only the one identity.
pub fn acquire_identity_at(
    config_path: &Path,
    env: &HashMap<String, String>,
) -> RuntimeResult<AcquiredIdentity> {
    match try_lock_slot(config_path)? {
        Some(lock) => finish(config_path.to_path_buf(), 1, lock, env),
        None => Err(RuntimeError::Invalid(format!(
            "identity {} already in use by another process",
            config_path.display()
        ))),
    }
}

/// Whether the environment pins the daemon to one specific identity.
///
/// Any of these means the operator chose the address, so a collision must be
/// reported rather than worked around. See the module docs for why
/// `TINYPLACE_SECRET_KEY` belongs in this list.
fn pins_identity(env: &HashMap<String, String>) -> bool {
    ["MEDULLA_HOME", "TINYPLACE_CONFIG", "TINYPLACE_SECRET_KEY"]
        .iter()
        .any(|key| {
            env.get(*key)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        })
}

/// Load or mint the identity for a slot whose lock is already held.
fn finish(
    config_path: PathBuf,
    slot: usize,
    lock: IdentityLock,
    env: &HashMap<String, String>,
) -> RuntimeResult<AcquiredIdentity> {
    let (signer, config) = load_or_create_identity(&config_path, env)?;
    let identity_dir = config_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(AcquiredIdentity {
        signer,
        config,
        config_path,
        identity_dir,
        slot,
        lock,
    })
}

/// Try to take the exclusive lock guarding the slot whose config lives at
/// `config_path`, creating the identity directory and the lock file if this slot
/// has never been used.
///
/// `Ok(None)` means the slot is held by another process (or another thread that
/// holds its own open handle) — the caller moves on. `Err` is reserved for a slot
/// that could not be *attempted*, e.g. an unwritable home.
fn try_lock_slot(config_path: &Path) -> RuntimeResult<Option<IdentityLock>> {
    let dir = config_path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let lock_path = dir.join(LOCK_FILE);
    let file: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(IdentityLock::new(file, lock_path))),
        Err(err) if is_contended(&err) => Ok(None),
        Err(err) => Err(RuntimeError::Io(err)),
    }
}

/// Whether a failed lock attempt means "someone else holds it" rather than a
/// real I/O failure.
///
/// `fs2` signals contention with a platform-specific errno — `EWOULDBLOCK` on
/// unix, `ERROR_LOCK_VIOLATION` on Windows — and exposes it as a sentinel error.
/// The raw code is compared rather than [`std::io::ErrorKind`] because the
/// Windows code has no dedicated kind and would otherwise be indistinguishable
/// from every other uncategorized failure, quietly turning a broken disk into a
/// silent fan-out to the next slot.
fn is_contended(err: &std::io::Error) -> bool {
    let sentinel = fs2::lock_contended_error();
    match (err.raw_os_error(), sentinel.raw_os_error()) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => err.kind() == sentinel.kind(),
    }
}
