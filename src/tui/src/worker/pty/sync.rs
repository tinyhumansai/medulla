//! Poison-tolerant lock helpers.
//!
//! Every lock in this layer used to be `.lock().unwrap()`. That turns one panic
//! anywhere — a reader thread, a render pass, `vt100`'s own documented
//! `set_scrollback` underflow — into a permanently poisoned mutex, and every
//! later caller panics on it in turn. The TUI dies of a fault in one session.
//!
//! A poisoned lock here means "some thread panicked while holding this", not
//! "the data is unusable": the guarded values are a terminal emulator, a pty
//! handle, and a couple of strings, none of which have an invariant a panic
//! could half-break in a way the next reader would care about. So the guard is
//! taken either way and the session carries on.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Lock `mutex`, ignoring poisoning.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Read-lock `lock`, ignoring poisoning.
pub(super) fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Write-lock `lock`, ignoring poisoning.
pub(super) fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
