//! The tasks this control plane has dispatched, and their outcomes.
//!
//! Dispatch is handle-and-poll rather than a blocking call, so something has to
//! own the work between "accepted" and "read". That is this module.
//!
//! Why handle-and-poll at all: [`crate::hub::TaskRunner::run`] owns no
//! wall-clock deadline — a harness that keeps making progress is left to finish
//! however long it takes — while an MCP client imposes its own per-call timeout.
//! Under a blocking design the client gives up while the dispatch keeps running,
//! leaving a task nobody has an id for and nobody can abort. A handle is also
//! the only way `fleet_abort` can exist, since
//! [`abort_task`](crate::hub::TaskRunner::abort_task) keys on an id we must have
//! handed back.
//!
//! Everything here is bounded. An orchestrator session runs for days, so an
//! unbounded map of finished tasks and their status transcripts is a slow leak.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::hub::{RunError, TaskRequest};

use super::super::types::FleetOps;
use super::types::{SpawnError, TaskEntry, TaskRegistry, TaskState, Tracked};

/// The most status lines kept per task.
///
/// A chatty harness emits thousands; a poller only ever reads the recent tail,
/// and the rest is the operator's transcript to keep, not ours.
const STATUS_TAIL_MAX: usize = 200;

/// How long a settled task's result stays readable.
const RESULT_TTL: Duration = Duration::from_secs(30 * 60);

/// The process-wide ceiling on live dispatches across every grant.
pub(super) const MAX_IN_FLIGHT_GLOBAL: usize = 256;

/// The most live and retained settled tasks kept in the registry.
pub(super) const MAX_ENTRIES: usize = MAX_IN_FLIGHT_GLOBAL * 2;

/// Translate a dispatch failure into a state a model can reason about.
///
/// The distinction that matters: `Busy` and `Held` mean *nothing was attempted*
/// — the worker shed the task or a person holds the harness — where `Worker` is
/// the task's own failure. A caller that cannot tell them apart either abandons
/// work that never started or retries a genuine error forever.
fn state_from_error(error: RunError) -> TaskState {
    let (status, message, retryable) = match error {
        RunError::Timeout => (
            "timeout",
            "the worker stopped reporting progress and the dispatch was reaped".to_string(),
            true,
        ),
        RunError::Aborted => (
            "aborted",
            "the task was cancelled; anything it already wrote to disk is still written"
                .to_string(),
            false,
        ),
        RunError::Worker(message) => ("failed", message, false),
        RunError::Busy(message) => (
            "busy",
            format!("{message} — nothing was attempted, so this is worth retrying"),
            true,
        ),
        RunError::Held(message) => (
            "held",
            format!("{message} — a person is working there; try another worker"),
            true,
        ),
        RunError::Transport(message) => ("failed", message, true),
        // No catch-all arm: `RunError` is `#[non_exhaustive]`, but that only
        // obliges *downstream* crates to write one, and this is the same crate.
        // Matching every variant is what we want anyway — a variant added later
        // should fail this build and be given a status and a retryability,
        // rather than quietly folding into "failed, do not retry".
    };
    TaskState::Failed {
        status,
        message,
        retryable,
    }
}

impl TaskRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept `request` on behalf of `token` and return its handle.
    ///
    /// Returns as soon as the task is recorded; the dispatch itself runs on a
    /// spawned task and writes its terminal state back here. `task_id` is taken
    /// from the request rather than minted here so the caller — which also set
    /// `abort_id` — keeps the two consistent.
    pub(super) async fn spawn_below_limit(
        &self,
        ops: Arc<dyn FleetOps>,
        token: String,
        request: TaskRequest,
        max_in_flight: usize,
    ) -> Result<String, SpawnError> {
        let task_id = request.abort_id.clone();
        let (settled, _) = watch::channel(false);
        let (status_tx, mut status_rx) = mpsc::unbounded_channel::<String>();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();

        let entry = TaskEntry {
            task_id: task_id.clone(),
            token,
            worker: request.worker_address.clone(),
            instruction: crate::logging::preview(&request.instruction),
            started_at: crate::clock::now_millis(),
            finished_at: None,
            state: TaskState::Running,
            status_tail: Vec::new(),
        };

        {
            let mut tasks = self.inner.lock().map_err(|_| SpawnError::Unavailable)?;
            Self::evict(&mut tasks);
            let global_in_flight = tasks
                .values()
                .filter(|tracked| !tracked.entry.state.is_settled())
                .count();
            if global_in_flight >= MAX_IN_FLIGHT_GLOBAL {
                return Err(SpawnError::GlobalAtCapacity(global_in_flight));
            }
            let in_flight = tasks
                .values()
                .filter(|tracked| {
                    tracked.entry.token == entry.token && !tracked.entry.state.is_settled()
                })
                .count();
            if in_flight >= max_in_flight {
                return Err(SpawnError::AtCapacity(in_flight));
            }
            tasks.insert(
                task_id.clone(),
                Tracked {
                    entry,
                    settled: settled.clone(),
                },
            );
        }

        // Status lines are folded in as they arrive rather than collected at the
        // end, so a poll against a still-running task can show progress instead
        // of only "still running".
        let status_registry = self.clone();
        let status_id = task_id.clone();
        tokio::spawn(async move {
            while let Some(line) = status_rx.recv().await {
                status_registry.push_status(&status_id, line);
            }
        });

        let done_registry = self.clone();
        let done_id = task_id.clone();
        tokio::spawn(async move {
            use std::future::Future;

            let mut dispatch = Box::pin(ops.dispatch(request, Some(status_tx)));
            // Poll through the dispatch's synchronous setup before publishing
            // its handle. HubFleetOps reaches TaskRunner's abort registration
            // before its first Pending, closing the dispatch-then-abort race.
            let immediate = std::future::poll_fn(|cx| match dispatch.as_mut().poll(cx) {
                std::task::Poll::Ready(outcome) => std::task::Poll::Ready(Some(outcome)),
                std::task::Poll::Pending => std::task::Poll::Ready(None),
            })
            .await;
            let _ = started_tx.send(());
            let outcome = match immediate {
                Some(outcome) => outcome,
                None => dispatch.await,
            };
            let state = match outcome {
                Ok(outcome) => TaskState::Done(Box::new(outcome)),
                Err(error) => state_from_error(error),
            };
            // The state is written before the signal, so anything woken by it
            // reads the settled entry rather than racing back to `Running`.
            done_registry.settle(&done_id, state);
            // `send_replace` rather than `send`: with no poller parked yet there
            // are no receivers, and `send` would report that as a failure and
            // leave the value unset for whoever subscribes next.
            settled.send_replace(true);
        });

        // A returned handle is now cancellable: the production dispatch has
        // registered its abort signal, or it already settled synchronously.
        if started_rx.await.is_err() {
            self.settle(
                &task_id,
                state_from_error(RunError::Transport(
                    "the dispatch task ended before it started".to_string(),
                )),
            );
        }
        Ok(task_id)
    }

    /// Append one status line to a task's tail, dropping the oldest past the cap.
    fn push_status(&self, task_id: &str, line: String) {
        if let Ok(mut tasks) = self.inner.lock() {
            if let Some(tracked) = tasks.get_mut(task_id) {
                let tail = &mut tracked.entry.status_tail;
                if tail.len() >= STATUS_TAIL_MAX {
                    let mut queue: VecDeque<String> = std::mem::take(tail).into();
                    while queue.len() >= STATUS_TAIL_MAX {
                        queue.pop_front();
                    }
                    *tail = queue.into();
                }
                tail.push(line);
            }
        }
    }

    /// Record a task's terminal state.
    fn settle(&self, task_id: &str, state: TaskState) {
        if let Ok(mut tasks) = self.inner.lock() {
            if let Some(tracked) = tasks.get_mut(task_id) {
                tracked.entry.state = state;
                tracked.entry.finished_at = Some(crate::clock::now_millis());
            }
        }
    }

    /// Drop finished entries that have aged out, then oldest-finished-first if
    /// the table is still over its cap.
    ///
    /// A `Running` entry is never evicted: its handle is the only way anyone can
    /// abort it, and forgetting it would strand a live harness session.
    fn evict(tasks: &mut HashMap<String, Tracked>) {
        let now = crate::clock::now_millis();
        let ttl = RESULT_TTL.as_millis() as i64;
        tasks.retain(|_, tracked| {
            tracked
                .entry
                .finished_at
                .is_none_or(|finished| now - finished < ttl)
        });
        if tasks.len() < MAX_ENTRIES {
            return;
        }
        let mut finished: Vec<(String, i64)> = tasks
            .iter()
            .filter_map(|(id, tracked)| {
                tracked
                    .entry
                    .finished_at
                    .filter(|_| tracked.settled.receiver_count() == 0)
                    .map(|finished| (id.clone(), finished))
            })
            .collect();
        finished.sort_by_key(|(_, finished)| *finished);
        for (id, _) in finished
            .into_iter()
            .take(tasks.len().saturating_sub(MAX_ENTRIES) + 1)
        {
            tasks.remove(&id);
        }
    }

    /// One task, if `token` dispatched it.
    ///
    /// Scoped by grant so one harness can neither read nor poll another's work.
    pub fn get(&self, token: &str, task_id: &str) -> Option<TaskEntry> {
        let tasks = self.inner.lock().ok()?;
        let tracked = tasks.get(task_id)?;
        (tracked.entry.token == token).then(|| tracked.entry.clone())
    }

    /// Every task `token` dispatched, newest first.
    pub fn list(&self, token: &str) -> Vec<TaskEntry> {
        let Ok(tasks) = self.inner.lock() else {
            return Vec::new();
        };
        let mut entries: Vec<TaskEntry> = tasks
            .values()
            .filter(|tracked| tracked.entry.token == token)
            .map(|tracked| tracked.entry.clone())
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.started_at));
        entries
    }

    /// How many of `token`'s tasks are still running.
    pub fn in_flight(&self, token: &str) -> usize {
        let Ok(tasks) = self.inner.lock() else {
            return 0;
        };
        tasks
            .values()
            .filter(|tracked| tracked.entry.token == token && !tracked.entry.state.is_settled())
            .count()
    }

    /// Wait up to `timeout` for a task to settle, then return it either way.
    ///
    /// A timeout is not an error: it answers "still running" plus whatever
    /// progress has accumulated, which is what lets a caller poll inside its own
    /// client's per-call timeout without burning tokens on a spin loop.
    pub async fn wait(&self, token: &str, task_id: &str, timeout: Duration) -> Option<TaskEntry> {
        let (entry, mut settled) = {
            let tasks = self.inner.lock().ok()?;
            let tracked = tasks.get(task_id)?;
            if tracked.entry.token != token {
                return None;
            }
            // Subscribing while the lock is held is what closes the race: a
            // settle that lands after this cannot be missed, because the
            // receiver retains the value rather than only catching live wakeups.
            (tracked.entry.clone(), tracked.settled.subscribe())
        };
        if entry.state.is_settled() || timeout.is_zero() {
            return Some(entry);
        }
        let _ = tokio::time::timeout(timeout, settled.wait_for(|settled| *settled)).await;
        self.get(token, task_id)
    }

    /// The abort id for a task `token` owns, if it owns one.
    ///
    /// Resolved through the registry rather than taken from the request so a
    /// caller can only ever abort work minted for its own grant.
    pub fn abort_id_for(&self, token: &str, task_id: &str) -> Option<String> {
        self.get(token, task_id).map(|entry| entry.task_id)
    }
}
