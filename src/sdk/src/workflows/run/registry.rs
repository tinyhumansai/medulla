//! The in-flight run registry.
//!
//! A run is cancelled by id, from somewhere else entirely — an abort frame, a
//! keypress, a shutdown — so the thing that can stop it has to be findable
//! while it is running and gone the moment it is not. This is that map, with an
//! RAII guard so no exit path can forget to clean up.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::Notify;

use crate::workflows::RunId;

/// Cancellation signals for runs currently executing in this process.
type Registry = Mutex<HashMap<RunId, Arc<Notify>>>;

/// The process-wide registry.
fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A registered run, deregistered when dropped.
///
/// Held for the whole run, including every path that unwinds out of it, so a
/// panicking or abandoned run cannot leave a signal behind for a later run that
/// happens to reuse its id.
pub struct RunGuard {
    id: RunId,
    signal: Arc<Notify>,
}

impl RunGuard {
    /// Register `id` and return its guard plus the signal to await.
    ///
    /// Call this *before* the run starts, not inside it: a cancel that arrives
    /// while the run is still being set up must find something to cancel, or it
    /// is silently lost.
    pub fn register(id: &str) -> (Self, Arc<Notify>) {
        let signal = Arc::new(Notify::new());
        registry()
            .lock()
            .expect("run registry lock")
            .insert(id.to_string(), signal.clone());
        (
            Self {
                id: id.to_string(),
                signal: signal.clone(),
            },
            signal,
        )
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut runs = match registry().lock() {
            Ok(runs) => runs,
            // A poisoned registry means another run panicked. Leaving this
            // entry behind is worse than doing nothing, but there is nothing
            // safe to do about it here.
            Err(_) => return,
        };
        // Only remove our own entry: an id reused by a later run must not have
        // its signal torn out by this guard's drop.
        if runs
            .get(&self.id)
            .is_some_and(|existing| Arc::ptr_eq(existing, &self.signal))
        {
            runs.remove(&self.id);
        }
    }
}

/// Cancel the run with `id`, if it is still running.
///
/// Returns whether a run was found. A cancel for a run that already settled is
/// not an error — the caller got what they wanted.
pub fn cancel(id: &str) -> bool {
    let runs = registry().lock().expect("run registry lock");
    match runs.get(id) {
        Some(signal) => {
            signal.notify_waiters();
            true
        }
        None => false,
    }
}

/// Whether a run is currently executing in this process.
pub fn is_running(id: &str) -> bool {
    registry()
        .lock()
        .expect("run registry lock")
        .contains_key(id)
}
