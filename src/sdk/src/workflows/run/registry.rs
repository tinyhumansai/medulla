//! The in-flight run registry.
//!
//! A run is cancelled by id, from somewhere else entirely — an abort frame, a
//! keypress, a shutdown — so the thing that can stop it has to be findable
//! while it is running and gone the moment it is not. This is that map, with an
//! RAII guard so no exit path can forget to clean up.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::Notify;

use crate::workflows::RunId;

/// A run's cancellation state.
///
/// The flag is what makes a cancel durable. `Notify::notify_waiters` wakes only
/// the tasks already waiting and stores no permit, so a cancel arriving between
/// the guard being claimed and the run reaching its `select!` would be dropped
/// on the floor — precisely the window claiming early was supposed to protect.
/// The flag records that it happened; the notify only wakes whoever is asleep.
#[derive(Debug, Default)]
pub struct CancelSignal {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelSignal {
    /// Mark the run cancelled and wake anyone waiting.
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Whether a cancel has already been signalled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Resolve as soon as the run is cancelled, now or later.
    ///
    /// Returns immediately for a cancel that arrived before this was awaited,
    /// which is the whole point of the flag.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        // Register interest *before* re-checking: a cancel landing between the
        // check above and the await would otherwise be missed too.
        let waiting = self.notify.notified();
        if self.is_cancelled() {
            return;
        }
        waiting.await;
    }
}

/// Cancellation signals for runs currently executing in this process.
type Registry = Mutex<HashMap<RunId, Arc<CancelSignal>>>;

/// The process-wide registry.
fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A run's registration: the guard that holds it, and the signal to await.
///
/// The two travel together because holding one without the other is always a
/// bug — a guard whose signal nobody watches registers a run that cannot
/// actually be stopped.
pub type RunClaim = (RunGuard, Arc<CancelSignal>);

/// A registered run, deregistered when dropped.
///
/// Held for the whole run, including every path that unwinds out of it, so a
/// panicking or abandoned run cannot leave a signal behind for a later run that
/// happens to reuse its id.
pub struct RunGuard {
    id: RunId,
    signal: Arc<CancelSignal>,
}

impl RunGuard {
    /// The run id this guard holds.
    ///
    /// A claim can be taken out by one caller and adopted by another (see
    /// [`crate::workflows::RunContext::claim`]), and the adopter has to be able
    /// to check it is being handed the claim for the run it is about to start.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Register `id` and return its guard plus the signal to await.
    ///
    /// Call this *before* the run starts, not inside it: a cancel that arrives
    /// while the run is still being set up must find something to cancel, or it
    /// is silently lost.
    pub fn register(id: &str) -> (Self, Arc<CancelSignal>) {
        let signal = Arc::new(CancelSignal::default());
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

    /// Register `id` only if no run is executing under it.
    ///
    /// The claim and the check happen under one lock, which is what makes this
    /// safe against two frames arriving together — a `is_running` test followed
    /// by a `register` would let both through the gap. Returns `None` when the
    /// id is taken, which the caller should report rather than run.
    pub fn claim(id: &str) -> Option<(Self, Arc<CancelSignal>)> {
        let mut runs = registry().lock().expect("run registry lock");
        if runs.contains_key(id) {
            return None;
        }
        let signal = Arc::new(CancelSignal::default());
        runs.insert(id.to_string(), signal.clone());
        Some((
            Self {
                id: id.to_string(),
                signal: signal.clone(),
            },
            signal,
        ))
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
            signal.cancel();
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
