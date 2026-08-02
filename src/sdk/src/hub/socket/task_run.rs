//! Serving one `medulla:task_run`: resolve the worker, relay the instruction,
//! and answer with exactly one terminal `medulla:task_result`.
//!
//! Split out of [`super`] so the terminal frame can be built — and asserted on —
//! without standing up a Socket.IO server. The retry policy a failed task
//! reports back is the hub's most consequential single decision, and
//! [`result_frame`] is pure for exactly that reason.

use std::sync::Arc;

use rust_socketio::asynchronous::Client;
use rust_socketio::Payload;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::super::roster::{address_of, SharedRoster, SharedSubscriptionStrategy};
use super::super::runner::TaskRunner;
use super::super::types::{HubLog, RunError, TaskOutcome, TaskRequest};
use super::super::ActivityLog;
use super::{first_obj, str_field, wire_task_id};

/// Whether the orchestrator should re-dispatch a failed task.
///
/// Infra-shaped failures are retryable so medulla re-runs; a clean worker error
/// is terminal — re-running work the harness actually attempted and rejected
/// only burns the same tokens twice.
///
/// Backpressure counts as infra-shaped. A worker that refused the task because
/// it was already holding its maximum pending tasks never attempted it, and
/// reporting "permanently failed" for a message that literally says "retry
/// later" is the one answer that is certainly wrong. medulla bounds the
/// re-dispatch with its own attempt ceiling and exponential backoff, so this
/// cannot become a hot loop against a saturated worker.
/// A workspace an operator is sitting in counts too. Nothing was attempted, and
/// the harness will be usable again the moment they hand it back — so the task
/// is deferred, not failed. It carries a `reason` on the wire (see
/// [`result_frame`]) precisely so the orchestrator can route elsewhere instead
/// of waiting on a person.
pub(in crate::hub) fn is_retryable(err: &RunError) -> bool {
    matches!(
        err,
        RunError::Timeout | RunError::Transport(_) | RunError::Busy(_) | RunError::Held(_)
    )
}

/// How long the backend is advised to wait before re-dispatching a task that was
/// refused because a person holds the workspace.
///
/// Advisory only, and deliberately coarse: this is a human timescale, not a
/// queue-depth one. It exists so a held workspace does not turn into a tight
/// re-dispatch loop against someone who has stepped away from their desk.
const HELD_RETRY_AFTER_MS: u64 = 60_000;

/// The terminal `medulla:task_result` body for one settled dispatch.
///
/// Pure, and lifted out of [`handle_task_run`] deliberately: what a failed task
/// tells the orchestrator about retrying is the difference between work that
/// resumes and work that is silently abandoned, and until this was a function it
/// could only be exercised by connecting a real socket.
///
/// Borrows the outcome rather than consuming it so the caller can narrate it
/// first; the reply is cloned into the frame, which costs one allocation on a
/// path that is already building a JSON document.
pub(in crate::hub) fn result_frame(
    task_id: &str,
    outcome: &Result<TaskOutcome, RunError>,
) -> Value {
    match outcome {
        Ok(outcome) => json!({
            "taskId": task_id,
            "ok": true,
            "reply": outcome.reply,
            "usage": {
                "inputTokens": outcome.usage.input_tokens,
                "outputTokens": outcome.usage.output_tokens,
            },
        }),
        // A held workspace is the one failure the orchestrator can act on
        // *specifically*, so it is the one that names itself. `reason` and
        // `retryAfterMs` are additive and ignorable: a backend that has never
        // heard of either still reads this as an ordinary retryable failure.
        Err(err @ RunError::Held(_)) => json!({
            "taskId": task_id,
            "ok": false,
            "error": err.to_string(),
            "retryable": true,
            "reason": "harnessHeld",
            "retryAfterMs": HELD_RETRY_AFTER_MS,
        }),
        Err(err) => json!({
            "taskId": task_id,
            "ok": false,
            "error": err.to_string(),
            "retryable": is_retryable(err),
        }),
    }
}

/// Relay one `task_run` to its worker and emit the terminal `task_result`.
pub(super) async fn handle_task_run(
    payload: Payload,
    socket: Client,
    runner: Arc<TaskRunner>,
    roster: SharedRoster,
    subscription_strategy: SharedSubscriptionStrategy,
    log: HubLog,
    activity: Option<ActivityLog>,
) {
    let Some(obj) = first_obj(payload) else {
        return;
    };
    let Some(task_id) = str_field(&obj, "taskId") else {
        return;
    };
    let instruction = str_field(&obj, "instruction").unwrap_or_default();
    let cycle_id = str_field(&obj, "cycleId");
    let agent_id = str_field(&obj, "agentId").unwrap_or_default();
    let requested_provider = str_field(&obj, "provider")
        .as_deref()
        .and_then(crate::tinyplace::HarnessProvider::from_wire);
    let custom_harness = str_field(&obj, "customHarness").or_else(|| str_field(&obj, "harnessId"));
    let model = str_field(&obj, "model");
    // The frame's own `timeoutMs` is deliberately ignored: that is the BACKEND's
    // task deadline, and the backend now enforces it (aborting a running task via
    // `medulla:task_abort`, which the hub relays). The hub owns no task deadline —
    // it only reaps a dispatch that goes silent (see `TaskRunner`'s idle window).

    // Resolve the address, then drop the lock before any await (the std guard is
    // not held across suspension points). An empty roster ⇒ nothing to run.
    let (worker_address, resolved_id, known) = {
        let r = roster.lock().expect("roster lock");
        let known: Vec<String> = r.iter().map(|w| w.id.clone()).collect();
        let addr = address_of(&r, &agent_id);
        // The roster id this resolved to, which is the lane the Agents view
        // groups the task under — not the raw `agentId`, which may be absent.
        let id = addr
            .as_ref()
            .and_then(|a| r.iter().find(|w| &w.address == a).map(|w| w.id.clone()))
            .unwrap_or_default();
        (addr, id, known)
    };
    let Some(worker_address) = worker_address else {
        // Say which of the two it is. "No workers" and "no worker by that name"
        // call for completely different actions, and reporting the first for
        // the second sent an operator looking for a connection problem that was
        // really a misaddressed task.
        let error = if known.is_empty() {
            "hub has no workers".to_string()
        } else {
            format!(
                "hub has no worker \"{agent_id}\" — known: {}",
                known.join(", ")
            )
        };
        (log)(&format!("hub: task {task_id} refused — {error}"));
        let _ = socket
            .emit(
                "medulla:task_result",
                json!({ "taskId": task_id, "ok": false, "error": error, "retryable": false }),
            )
            .await;
        return;
    };

    // An explicit provider is authoritative. Only an untargeted task consults
    // the subscription strategy, and a failed/unknown budget probe falls open
    // to the daemon's own configured default.
    let provider = match requested_provider {
        Some(provider) => Some(provider),
        None => {
            let strategy = *subscription_strategy
                .lock()
                .expect("subscription strategy lock");
            if strategy == crate::runtime::SubscriptionRoutingStrategy::Manual {
                None
            } else {
                match runner.capabilities(&worker_address).await {
                    Ok(capabilities) => {
                        super::super::roster::subscription_for_strategy(&capabilities, strategy)
                    }
                    Err(_) => None,
                }
            }
        }
    };

    let wire_task_id = wire_task_id(&task_id);

    // Attribute the task to the lane it will run on, before any frame comes
    // back — a frame that arrives before its dispatch is recorded would be
    // orphaned onto no worker at all.
    if let Some(activity) = &activity {
        activity.dispatched(&wire_task_id, &resolved_id);
    }

    // Forward `status` frames up as `task_envelope` while the task runs.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let status_socket = socket.clone();
    let status_task_id = task_id.clone();
    tokio::spawn(async move {
        while let Some(content) = rx.recv().await {
            let _ = status_socket
                .emit(
                    "medulla:task_envelope",
                    json!({
                        "taskId": status_task_id,
                        "envelope": { "kind": "status", "content": content },
                    }),
                )
                .await;
        }
    });

    // The instruction is on the line, not just the id. Two dispatches sharing a
    // task id are either two different pieces of work colliding on a name — ids
    // are assigned positionally per `delegate_tasks` call, so every call starts
    // again at `t1` — or the same work emitted twice. Those call for opposite
    // fixes, and the id alone cannot tell them apart.
    log(&format!(
        "hub: task_run {} (as {}) → {} · {}",
        task_id,
        wire_task_id,
        worker_address,
        crate::logging::preview(&instruction),
    ));

    let req = TaskRequest {
        task_id: wire_task_id.clone(),
        // The backend aborts by the ORIGINAL task id (`medulla:task_abort.taskId`),
        // not the per-dispatch wire id, so the runner registers its abort signal
        // under this.
        abort_id: task_id.clone(),
        cycle_id,
        instruction,
        worker_address,
        provider,
        custom_harness,
        model,
        // Forwarded rather than dropped: a worker advertises the workflows it
        // has installed, so the orchestrator naming one here is the other half
        // of that conversation. Blank is treated as absent so an emitter that
        // always writes the key still dispatches an ordinary instruction.
        tool_mode: None,
        workflow: obj
            .get("workflow")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string),
        workflow_inputs: obj
            .get("inputs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        // Not forwarded from the orchestrator. A backend-dispatched task is
        // discrete work, and honouring a conversation named on the wire would
        // let one caller's task resume a session opened for another's.
        conversation: None,
        fleet_depth: 0,
    };

    let outcome = runner.run(req, Some(tx)).await;
    match &outcome {
        Ok(o) => log(&format!(
            "hub: task {} ok ({} chars)",
            task_id,
            o.reply.len()
        )),
        Err(e) => log(&format!("hub: task {task_id} FAILED: {e}")),
    }

    let _ = socket
        .emit("medulla:task_result", result_frame(&task_id, &outcome))
        .await;
}
