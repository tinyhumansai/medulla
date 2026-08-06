//! What the hub currently sees of its workers' screens.
//!
//! The receiving half of `medulla.screen.v1`. The pump folds inbound frames in
//! here and the Agents view reads them out, which is the same shape as
//! [`ActivityLog`](super::ActivityLog): the hub already sees everything, so
//! recording it needs no backend change and no new protocol.
//!
//! Deliberately a small, bounded cache rather than a history. A screen answers
//! "what is this worker doing *now*"; there is nothing to be learned from the
//! one it showed a minute ago, and a hub watching a fan-out must not accumulate
//! a framebuffer per worker forever.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::protocol::{apply_frame, ApplyOutcome, ScreenFrame, ScreenGrid, ScreenView};

/// How many screens to keep. Beyond this the least recently updated is dropped.
///
/// A watcher looks at one screen at a time; this is sized so switching between a
/// handful of workers does not force a resync every time, not so a fleet can be
/// held resident.
const CAPACITY: usize = 8;

/// One worker session's screen, as the hub currently holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedScreen {
    /// The worker's link address.
    pub worker: String,
    /// The task whose session this shows.
    pub task_id: String,
    /// The synchronised screen.
    pub grid: ScreenGrid,
    /// The sequence `grid` reflects.
    pub seq: i64,
    /// Epoch ms when the last frame was applied — what "last frame 0.4s ago"
    /// reads from, and the only way to tell a still screen from a dead stream.
    pub updated_at: i64,
}

/// The hub's live screens, keyed by worker and session.
///
/// Cheap to clone; every clone reads and writes the same map.
///
/// Holds two things, and the distinction matters: `screens` is what the hub has
/// *received*, `watching` is what it has *asked for*. They are not the same, and
/// inferring one from the other is what let a stopped stream restart itself — a
/// frame still in flight when a watch ended found nothing held, which looks
/// exactly like a gap that needs repairing.
#[derive(Clone, Default)]
pub struct ScreenStore {
    screens: Arc<Mutex<HashMap<(String, String), WatchedScreen>>>,
    watching: Arc<Mutex<HashSet<(String, String)>>>,
}

impl ScreenStore {
    /// An empty store.
    pub fn new() -> Self {
        ScreenStore::default()
    }

    /// Fold `frame` from `worker` into the screen it belongs to.
    ///
    /// Returns [`ApplyOutcome::NeedsResync`] when the frame cannot be applied
    /// against what is held — a gap, a resize, or a first delta with no full
    /// frame before it. The caller must then ask the worker to resynchronise;
    /// the store deliberately does not, because it has no transport and
    /// swallowing the condition here would leave a view silently frozen.
    ///
    /// A rejected frame leaves the existing screen untouched, so what is on
    /// display stays a screen the worker really showed rather than a partly
    /// patched one.
    pub fn apply(&self, worker: &str, frame: &ScreenFrame, now: i64) -> ApplyOutcome {
        let key = (worker.to_string(), frame.task_id.clone());
        let mut screens = self.screens.lock().expect("screen store lock");

        let mut view = screens.get(&key).map(|held| ScreenView {
            grid: held.grid.clone(),
            seq: held.seq,
        });
        let outcome = apply_frame(&mut view, frame);
        if outcome == ApplyOutcome::Applied {
            if let Some(view) = view {
                screens.insert(
                    key,
                    WatchedScreen {
                        worker: worker.to_string(),
                        task_id: frame.task_id.clone(),
                        grid: view.grid,
                        seq: view.seq,
                        updated_at: now,
                    },
                );
                evict_stalest(&mut screens);
            }
        }
        outcome
    }

    /// The screen held for one worker session, if any.
    pub fn get(&self, worker: &str, task_id: &str) -> Option<WatchedScreen> {
        self.screens
            .lock()
            .expect("screen store lock")
            .get(&(worker.to_string(), task_id.to_string()))
            .cloned()
    }

    /// Every screen held for `worker`, most recently updated first.
    pub fn for_worker(&self, worker: &str) -> Vec<WatchedScreen> {
        let mut found: Vec<WatchedScreen> = self
            .screens
            .lock()
            .expect("screen store lock")
            .values()
            .filter(|held| held.worker == worker)
            .cloned()
            .collect();
        found.sort_by_key(|held| std::cmp::Reverse(held.updated_at));
        found
    }

    /// Everything held, most recently updated first.
    pub fn snapshot(&self) -> Vec<WatchedScreen> {
        let mut all: Vec<WatchedScreen> = self
            .screens
            .lock()
            .expect("screen store lock")
            .values()
            .cloned()
            .collect();
        all.sort_by_key(|held| std::cmp::Reverse(held.updated_at));
        all
    }

    /// Record that the hub has asked `worker` to stream `task_id`.
    ///
    /// Called when the subscribe is sent, not when the first frame lands: the
    /// gap between the two is a second of relay latency, and frames arriving in
    /// it are wanted.
    pub fn arm(&self, worker: &str, task_id: &str) {
        self.watching
            .lock()
            .expect("screen store lock")
            .insert((worker.to_string(), task_id.to_string()));
    }

    /// Whether the hub is currently watching `task_id` on `worker`.
    pub fn is_watching(&self, worker: &str, task_id: &str) -> bool {
        self.watching
            .lock()
            .expect("screen store lock")
            .contains(&(worker.to_string(), task_id.to_string()))
    }

    /// Stop watching, but keep the screen already received.
    ///
    /// The two are separate on purpose. Clearing the intent stops a late frame
    /// being read as a gap and answered with a fresh subscribe — the bug that
    /// made a stopped stream restart itself. Keeping the screen is what lets
    /// the pane redraw the moment the operator looks back: those frames were
    /// paid for on arrival, and a screen labelled with its age beats a blank
    /// pane waiting on a relay round trip.
    pub fn disarm(&self, worker: &str, task_id: &str) {
        self.watching
            .lock()
            .expect("screen store lock")
            .remove(&(worker.to_string(), task_id.to_string()));
    }

    /// Drop a screen and any intent to watch it — when its worker goes away, or
    /// its task is over and the screen could only ever go stale.
    pub fn forget(&self, worker: &str, task_id: &str) {
        let key = (worker.to_string(), task_id.to_string());
        self.watching
            .lock()
            .expect("screen store lock")
            .remove(&key);
        self.screens.lock().expect("screen store lock").remove(&key);
    }

    /// How many screens are held.
    pub fn len(&self) -> usize {
        self.screens.lock().expect("screen store lock").len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Drop the least recently updated screen while over capacity.
fn evict_stalest(screens: &mut HashMap<(String, String), WatchedScreen>) {
    while screens.len() > CAPACITY {
        let stalest = screens
            .iter()
            .min_by_key(|(_, held)| held.updated_at)
            .map(|(key, _)| key.clone());
        match stalest {
            Some(key) => {
                screens.remove(&key);
            }
            None => return,
        }
    }
}

#[cfg(test)]
mod tests;
