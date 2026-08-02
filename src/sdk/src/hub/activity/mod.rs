//! What the hub's workers are actually doing, recorded as it happens.
//!
//! The orchestrator's Agents view derives per-worker activity from the render
//! snapshot's event log, and that log is filled from the *backend's* SSE stream
//! — whose vocabulary (`user`, `assistant`, `cycle_*`, the deltas) contains
//! nothing about delegated tasks. So a worker could be dispatched to, stream
//! tool activity for minutes, and reply, while the tab showed it idle: the
//! events existed, in this process, and simply had nowhere to go.
//!
//! This is that missing surface. The hub already sees the whole lifecycle — it
//! dispatches the task and every frame comes back through its inbox pump — so
//! recording it here needs no backend change and no new protocol. It is
//! deliberately a *ring*, not a history: this exists to answer "what is
//! happening now", and a worker left running for a week must not accumulate its
//! entire past in memory because nobody was looking at the screen.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// How many activity records to retain before dropping the oldest.
///
/// Sized for a wide fan-out (dozens of tasks, several frames each) with room to
/// spare, while staying far below anything that would matter for memory.
const CAPACITY: usize = 512;

/// How many task→worker attributions to remember.
///
/// Bounded separately because a task's frames can arrive long after dispatch,
/// and losing the attribution would orphan them onto no lane at all.
const ATTRIBUTION_CAPACITY: usize = 512;

impl ActivityLog {
    /// An empty log.
    pub fn new() -> Self {
        ActivityLog::default()
    }

    /// Record that `task_id` was dispatched to the worker with `agent_id`.
    ///
    /// Called at dispatch rather than inferred later: the hub resolves the
    /// target once, and re-deriving it when a frame returns would have to guess
    /// again with less information.
    pub fn dispatched(&self, task_id: &str, agent_id: &str) {
        self.dispatched_as(task_id, task_id, agent_id);
    }

    /// Correlate frames carrying `observed_task_id` with an operator-visible row.
    ///
    /// Most dispatches use one id for both. Control-plane dispatches do not:
    /// workers echo a unique dedupe id while the operator cancels by a separate
    /// abort handle. Recording the mapping here keeps ACKs and structured work
    /// on the visible, cancellable row without changing the wire protocol.
    pub fn dispatched_as(&self, observed_task_id: &str, activity_task_id: &str, agent_id: &str) {
        let mut map = self.attribution.lock().expect("attribution lock");
        map.retain(|entry| entry.observed_task_id != observed_task_id);
        map.push_back(ActivityAttribution {
            observed_task_id: observed_task_id.to_string(),
            activity_task_id: activity_task_id.to_string(),
            agent_id: agent_id.to_string(),
        });
        while map.len() > ATTRIBUTION_CAPACITY {
            map.pop_front();
        }
    }

    /// Record one frame observed for `task_id`, with no work snapshot attached.
    pub fn observed(&self, task_id: &str, kind: &str, content: &str, at: i64) {
        self.observed_with_work(task_id, kind, content, at, None);
    }

    /// Record one frame observed for `task_id`, carrying whatever the worker
    /// said it was working on.
    pub fn observed_with_work(
        &self,
        task_id: &str,
        kind: &str,
        content: &str,
        at: i64,
        work: Option<Box<crate::harness_work::WorkSnapshot>>,
    ) {
        let attribution = self
            .attribution
            .lock()
            .expect("attribution lock")
            .iter()
            .rev()
            .find(|entry| entry.observed_task_id == task_id)
            .cloned();
        let (task_id, agent_id) = attribution
            .map(|entry| (entry.activity_task_id, entry.agent_id))
            .unwrap_or_else(|| (task_id.to_string(), String::new()));
        let mut entries = self.entries.lock().expect("activity lock");
        entries.push_back(WorkerActivity {
            agent_id,
            task_id,
            kind: kind.to_string(),
            content: content.to_string(),
            at,
            work,
        });
        while entries.len() > CAPACITY {
            entries.pop_front();
        }
    }

    /// Everything retained, oldest first.
    pub fn snapshot(&self) -> Vec<WorkerActivity> {
        self.entries
            .lock()
            .expect("activity lock")
            .iter()
            .cloned()
            .collect()
    }

    /// Whether a reply or error has already settled the named task.
    ///
    /// Accepts the worker-facing id and resolves it through the same attribution
    /// map as [`observed`](Self::observed), so fallback producers cannot append
    /// a second terminal frame to an operator-visible alias.
    pub fn has_terminal(&self, observed_task_id: &str) -> bool {
        let task_id = self
            .attribution
            .lock()
            .expect("attribution lock")
            .iter()
            .rev()
            .find(|entry| entry.observed_task_id == observed_task_id)
            .map(|entry| entry.activity_task_id.clone())
            .unwrap_or_else(|| observed_task_id.to_string());
        self.entries
            .lock()
            .expect("activity lock")
            .iter()
            .any(|entry| {
                entry.task_id == task_id && matches!(entry.kind.as_str(), "reply" | "error")
            })
    }

    /// How many distinct tasks are still running per worker.
    ///
    /// A task counts as running once dispatched and stops the moment a terminal
    /// frame (`reply`/`error`) arrives — the same rule the peer sees, so the
    /// screen and the peer never disagree about whether work is outstanding.
    pub fn running_by_agent(&self) -> HashMap<String, Vec<String>> {
        let mut terminal: HashMap<String, bool> = HashMap::new();
        let mut agent_of: HashMap<String, String> = HashMap::new();
        for entry in self.entries.lock().expect("activity lock").iter() {
            agent_of.insert(entry.task_id.clone(), entry.agent_id.clone());
            let done = entry.kind == "reply" || entry.kind == "error";
            let slot = terminal.entry(entry.task_id.clone()).or_insert(false);
            *slot = *slot || done;
        }
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for (task_id, done) in terminal {
            if done {
                continue;
            }
            let agent = agent_of.get(&task_id).cloned().unwrap_or_default();
            out.entry(agent).or_default().push(task_id);
        }
        out
    }
}

mod types;
use types::ActivityAttribution;
pub use types::ActivityLog;
pub use types::WorkerActivity;
