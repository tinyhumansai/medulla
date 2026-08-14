//! Background work a `spawn` node starts and a `gate` node collects.
//!
//! The engine ships a default [`TaskRunner`](tinyflows::caps::TaskRunner) that
//! schedules on tokio but has no capabilities of its own, so it echoes the spec
//! back as the task's result instead of performing it. That is the right answer
//! for the engine crate — it cannot know what a tool call *means* — and the
//! wrong one for a host: a graph whose `spawn` produced `{"spec": "tool", …}`
//! rather than the tool's output would look like it ran and be quietly useless.
//!
//! [`MedullaTaskRunner`] is this host's answer. It performs exactly what the
//! `spawn` node would have performed inline — a tool call, an HTTP request, or a
//! whole child graph — but off the super-step that asked for it, which is the
//! entire point of the node: the branch carries on while the work runs, and a
//! downstream `gate` turns the ticket back into a result.
//!
//! # Why a fresh capability bundle per task
//!
//! A task is handed a bundle built on demand from [`TaskCapabilities`] rather
//! than a shared one. Sharing would mean the bundle held the runner (as
//! `Capabilities::tasks`) while the runner held the bundle — an `Arc` cycle, so
//! every run would leak its capabilities for the life of the process. Building
//! per task costs one bundle per spawned task, against work that is a whole
//! coding session or a child workflow, and it is what lets a child graph's own
//! `spawn` nodes overlap in turn: the bundle it gets has a task runner too.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tinyflows::caps::{Capabilities, TaskRunner, TaskSpec, TaskState};
use tinyflows::error::{EngineError, Result};

use crate::flow_engine::execute::{compile, run_detached};

/// Where the runner gets a capability bundle, and how long a task may take.
///
/// A trait rather than the concrete assembly so this module never has to know
/// how a bundle is put together — that stays the one job of the parent module.
pub trait TaskCapabilities: Send + Sync {
    /// A bundle for one spawned task, complete with its own task runner so a
    /// child graph's `spawn` nodes overlap the same way this one's did.
    fn capabilities(&self) -> Capabilities;

    /// The deadline a single spawned task runs under.
    ///
    /// A spawned task outlives the super-step that started it, so nothing else
    /// in the run will ever notice it wedged. Without a bound of its own it
    /// would sit in the process until the daemon restarted.
    fn timeout(&self) -> Duration;
}

/// A [`TaskRunner`] that actually performs the work a `spawn` node describes.
pub struct MedullaTaskRunner {
    /// How a task gets its capabilities and its deadline.
    caps: Arc<dyn TaskCapabilities>,
    /// Issued tickets and the state each has reached. A `gate` polls these
    /// repeatedly, so a settled result must stay readable rather than be
    /// consumed by the first poll.
    tasks: Mutex<HashMap<String, Arc<Mutex<TaskState>>>>,
    /// Abort handles, so a cancel reaches work that is still running.
    ///
    /// `Arc`-wrapped, unlike `tasks`, because the spawned task itself removes
    /// its own entry once it settles — an abort handle for finished work has no
    /// further use, and leaving it would grow this table for the life of the
    /// run on a wide `scatter` fan-out. `tasks` keeps every settled result on
    /// purpose, for repeated polling; only the handle is disposable.
    handles: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
    /// Ticket counter. Ids need to be unique within this runner and nothing
    /// more, so a counter beats a clock or a random source.
    next_id: Mutex<u64>,
}

impl std::fmt::Debug for MedullaTaskRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MedullaTaskRunner").finish_non_exhaustive()
    }
}

impl MedullaTaskRunner {
    /// A runner with no tasks, drawing capabilities from `caps`.
    pub fn new(caps: Arc<dyn TaskCapabilities>) -> Self {
        Self {
            caps,
            tasks: Mutex::new(HashMap::new()),
            handles: Arc::new(Mutex::new(HashMap::new())),
            next_id: Mutex::new(0),
        }
    }

    /// The next ticket id. Opaque to the graph: a `gate` passes it back, never
    /// interprets it.
    fn issue_ticket(&self) -> String {
        let mut next = self.next_id.lock().expect("ticket counter poisoned");
        *next += 1;
        format!("medulla-task-{next}")
    }

    /// The state cell for `ticket`, or an error naming the unknown ticket.
    fn state_of(&self, ticket: &str) -> Result<Arc<Mutex<TaskState>>> {
        self.tasks
            .lock()
            .expect("task table poisoned")
            .get(ticket)
            .cloned()
            .ok_or_else(|| EngineError::Capability(format!("unknown task ticket {ticket:?}")))
    }
}

/// Perform one task spec against `caps`.
///
/// The same three shapes the `spawn` node runs inline when no runner is
/// injected, so a graph produces the same answer either way and only the
/// overlap differs.
async fn perform(spec: TaskSpec, caps: &Capabilities) -> Result<Value> {
    match spec {
        TaskSpec::Tool { slug, args } => caps.tools.invoke(&slug, args, None).await,
        TaskSpec::Http { request } => caps.http.request(request, None).await,
        TaskSpec::Workflow { graph, input } => {
            let child: tinyflows::model::WorkflowGraph = serde_json::from_value(graph)
                .map_err(|err| EngineError::Capability(format!("spawned workflow: {err}")))?;
            let compiled = compile(&child).map_err(EngineError::Capability)?;
            let outcome = run_detached(&compiled, input, caps)
                .await
                .map_err(EngineError::Capability)?;
            // A detached child has no thread id, so nobody can approve it and
            // nobody can resume it. Parking is therefore terminal, and the
            // partial state it comes back with is not the answer a `gate`
            // asked for: without this check it would settle as `Done` with
            // that partial output, and the parent graph would carry on as
            // though the child had actually finished.
            if !outcome.pending_approvals.is_empty() {
                return Err(EngineError::Capability(format!(
                    "spawned workflow parked awaiting approval at {} and cannot be resumed",
                    outcome.pending_approvals.join(", ")
                )));
            }
            if outcome.cancelled {
                return Err(EngineError::Capability(
                    "spawned workflow was cancelled".to_string(),
                ));
            }
            Ok(outcome.output)
        }
    }
}

#[async_trait]
impl TaskRunner for MedullaTaskRunner {
    async fn start(&self, spec: TaskSpec) -> Result<String> {
        // `tokio::spawn` panics rather than erroring when no runtime is running,
        // and this capability is reachable from a host embedding the SDK on
        // another executor. Check first so that host gets a capability error it
        // can act on instead of losing the process.
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(EngineError::Capability(
                "spawn needs a running tokio runtime to start background work".to_string(),
            ));
        }

        let ticket = self.issue_ticket();
        let state = Arc::new(Mutex::new(TaskState::Pending));
        self.tasks
            .lock()
            .expect("task table poisoned")
            .insert(ticket.clone(), state.clone());

        let caps = self.caps.clone();
        let deadline = caps.timeout();
        let label = ticket.clone();
        let handle = tokio::spawn(async move {
            *state.lock().expect("task state poisoned") = TaskState::Running;
            // Built inside the task, not before it: assembling a bundle opens a
            // state store and an HTTP client, and the branch that spawned this
            // is already running again.
            let bundle = caps.capabilities();
            let settled = match tokio::time::timeout(deadline, perform(spec, &bundle)).await {
                Ok(Ok(value)) => TaskState::Done(value),
                // A task that starts and then fails settles as `Failed` rather
                // than propagating: the `spawn` already returned, so the only
                // place left to report it is the gate that collects the ticket,
                // which emits it as `{failed: true, error}` for the graph to
                // branch on.
                Ok(Err(err)) => TaskState::Failed(err.to_string()),
                Err(_) => TaskState::Failed(format!(
                    "spawned task {label} exceeded its {}s deadline",
                    deadline.as_secs()
                )),
            };
            *state.lock().expect("task state poisoned") = settled;
        });
        self.handles
            .lock()
            .expect("handle table poisoned")
            .insert(ticket.clone(), handle.abort_handle());
        Ok(ticket)
    }

    async fn poll(&self, ticket: &str) -> Result<TaskState> {
        let state = self.state_of(ticket)?;
        let state = state.lock().expect("task state poisoned").clone();
        Ok(state)
    }

    async fn cancel(&self, ticket: &str) -> Result<()> {
        let state = self.state_of(ticket)?;
        if let Some(handle) = self
            .handles
            .lock()
            .expect("handle table poisoned")
            .remove(ticket)
        {
            handle.abort();
        }
        let mut state = state.lock().expect("task state poisoned");
        // Never rewrite a settled result: a gate that already released on this
        // ticket has used the value, and a cancel arriving afterwards must not
        // retroactively turn a collected success into a failure.
        if !state.is_settled() {
            *state = TaskState::Failed("cancelled".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tasks_tests;
