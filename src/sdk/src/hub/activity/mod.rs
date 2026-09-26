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
//!
//! "What is happening now" is also why a *settled* task leaves. The ring's
//! capacity alone is not retirement: a task that replied an hour ago stays in it
//! until 512 later frames happen to push it out, and until they do the Sessions
//! rail draws a row for it — which is how an operator ends up reading a list of
//! a dozen finished dispatches to find the one still running. So a task with a
//! terminal frame is dropped outright once [`SETTLED_RETENTION_MS`] has passed
//! since its last frame, taking its attribution with it. The control plane's own
//! task registry has had exactly this rule (a settled entry ages out; a running
//! one never does) since it was written; this log simply never grew one.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use super::{RunError, TaskOutcome};

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

/// How long a settled task's records stay in the log after its last frame.
///
/// Long enough that an operator who was looking elsewhere when a dispatch ended
/// still finds the row and its outcome when they look back; short enough that a
/// session driving dispatches all afternoon does not turn the rail into a
/// scrollback of finished work. Deliberately far shorter than the control
/// plane's `RESULT_TTL`, which is a *readable result* a poller may still ask
/// for by id — this is a row on a screen, and a screen only owes the operator
/// what is happening now.
const SETTLED_RETENTION_MS: i64 = 2 * 60 * 1_000;

/// Whether a frame kind ends a task.
///
/// The same rule the peer sees, and the only definition of settled in this
/// module: `reply` and `error` are terminal, everything else is progress.
fn is_terminal_kind(kind: &str) -> bool {
    matches!(kind, "reply" | "error")
}

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
        // Retirement is driven by whoever reads the log, and a headless daemon
        // has no reader. Doing it here as well bounds the log on a host where
        // nothing is watching, at the cost of one pass per dispatch.
        self.retire_settled(crate::clock::now_millis());
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

    /// Record how a dispatch ended, when nothing else already has.
    ///
    /// A worker's own `reply`/`error` frame settles a task through the hub pump,
    /// and that is the ordinary path. Outcomes generated on *this* side never
    /// reach it: a transport failure, an abort, a dispatch the watchdog reaped
    /// for going silent, or a worker that was never reachable all produce a
    /// `Result` here and no frame anywhere. Without this the task's last recorded
    /// frame is whatever progress it managed, so it reads as `running` — on the
    /// screen and to [`running_by_agent`](Self::running_by_agent) — for as long
    /// as the process lives.
    ///
    /// Takes the worker-facing id and resolves it through the attribution map,
    /// exactly as [`observed`](Self::observed) does, so this cannot append a
    /// second terminal frame under an operator-visible alias.
    pub fn record_outcome(&self, observed_task_id: &str, outcome: &Result<TaskOutcome, RunError>) {
        if self.has_terminal(observed_task_id) {
            return;
        }
        let (kind, content) = match outcome {
            Ok(result) => ("reply", result.reply.clone()),
            Err(error) => ("error", error.to_string()),
        };
        self.observed(observed_task_id, kind, &content, crate::clock::now_millis());
    }

    /// Drop every record of a task that settled more than
    /// [`SETTLED_RETENTION_MS`] ago, and its attribution with it.
    ///
    /// `now` is passed in rather than read here so the rule can be exercised
    /// against a fixed clock.
    ///
    /// A task with no terminal frame is never retired however old it is, for the
    /// reason the control plane's registry never evicts a `Running` entry: it may
    /// still be executing, and taking it off the screen would hide live work
    /// (and, here, the row the operator cancels it from).
    pub fn retire_settled(&self, now: i64) {
        let retired: HashSet<String> = {
            let mut entries = self.entries.lock().expect("activity lock");
            let mut last_at: HashMap<&str, i64> = HashMap::new();
            let mut settled: HashSet<&str> = HashSet::new();
            for entry in entries.iter() {
                let seen = last_at.entry(entry.task_id.as_str()).or_insert(entry.at);
                *seen = (*seen).max(entry.at);
                if is_terminal_kind(&entry.kind) {
                    settled.insert(entry.task_id.as_str());
                }
            }
            // Aged from the task's *last* frame, not from the terminal one: a
            // late-arriving frame is still the task saying something, and
            // retiring it out from under a reader mid-conversation would be the
            // one way this rule could remove something worth seeing.
            let retired: HashSet<String> = settled
                .iter()
                .filter(|task_id| {
                    last_at
                        .get(*task_id)
                        .is_some_and(|last| now.saturating_sub(*last) >= SETTLED_RETENTION_MS)
                })
                .map(|task_id| (*task_id).to_string())
                .collect();
            if retired.is_empty() {
                return;
            }
            entries.retain(|entry| !retired.contains(&entry.task_id));
            retired
        };
        // Taken after the entries lock is released, never with it held: the
        // inbound path locks attribution first and then entries, and holding
        // both here in the opposite order is the whole recipe for a deadlock.
        self.attribution
            .lock()
            .expect("attribution lock")
            .retain(|entry| !retired.contains(&entry.activity_task_id));
    }

    /// Everything retained, oldest first.
    ///
    /// Retires what has aged out first, so a reader never sees a task the log
    /// has stopped keeping. This is the hook that makes retirement happen at all
    /// on an idle host: the rail reads this every frame, and a log nobody writes
    /// to has nothing else to drive it.
    pub fn snapshot(&self) -> Vec<WorkerActivity> {
        self.snapshot_at(crate::clock::now_millis())
    }

    /// [`snapshot`](Self::snapshot) against a caller-supplied clock.
    ///
    /// Splitting the clock out is what lets the retirement rule be exercised
    /// without waiting two minutes of wall time for it.
    pub fn snapshot_at(&self, now: i64) -> Vec<WorkerActivity> {
        self.retire_settled(now);
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
            .any(|entry| entry.task_id == task_id && is_terminal_kind(&entry.kind))
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
            let done = is_terminal_kind(&entry.kind);
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
