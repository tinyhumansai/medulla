//! The Socket.IO harness client — the hub's uplink to the hosted backend brain.
//!
//! Connects to the backend's harness plane, advertises the shared worker roster
//! (`medulla:register_agents`), and for every `medulla:task_run` the brain emits
//! it dispatches through the [`TaskRunner`] over tiny.place and streams the
//! result back (`medulla:task_result`, with `medulla:task_envelope` progress).
//! The roster is shared with the [`HubHandle`](super::HubHandle), so a worker
//! added at runtime is targetable and re-advertised immediately.

use std::sync::Arc;

use futures::FutureExt;
use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Event, Payload};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::roster::{address_of, register_payload, SharedRoster};
use super::runner::TaskRunner;
use super::types::{RunError, TaskRequest};

/// Monotonic suffix making each dispatch's worker-facing task id unique.
static DISPATCH_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The id the *worker* sees for this dispatch, unique per dispatch.
///
/// `delegate_tasks` names unnamed tasks positionally *per call* (`t${n}`), so
/// every call starts again at `t1`. The worker dedupes on sender + taskId, so a
/// second call's `t1` is refused as a duplicate of the first's — which is
/// entirely different work. Seen in the field: three dispatches all named `t1`,
/// carrying three different instructions, two of them refused.
///
/// Worker-facing only. Every frame sent back to the backend keeps the original
/// id, because that is the key its waiter is registered under.
pub(super) fn wire_task_id(task_id: &str) -> String {
    format!(
        "{task_id}#{}",
        DISPATCH_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// The first JSON object carried by a received event payload, if any.
fn first_obj(payload: Payload) -> Option<Value> {
    match payload {
        Payload::Text(mut values) => (!values.is_empty()).then(|| values.remove(0)),
        #[allow(deprecated)]
        Payload::String(s) => serde_json::from_str(&s).ok(),
        Payload::Binary(_) => None,
    }
}

/// A required, non-empty string field on a received object.
fn str_field(obj: &Value, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Connect to the backend harness plane and wire every down-event to the runner.
///
/// Authenticates with `jwt` in the Socket.IO handshake, advertises `roster` on
/// every (re)connect, and dispatches `medulla:task_run` frames through `runner`.
/// The hub owns no task deadline — the backend does — so nothing here bounds how
/// long a task may run; `runner` only reaps a dispatch that goes silent. Returns
/// the connected client (which the [`HubHandle`](super::HubHandle) re-emits
/// through); drop it to disconnect.
pub async fn connect_harness(
    backend_url: &str,
    jwt: &str,
    roster: SharedRoster,
    runner: Arc<TaskRunner>,
    log: super::types::HubLog,
    activity: Option<super::ActivityLog>,
) -> anyhow::Result<Client> {
    let connect_roster = roster.clone();
    let run_log = log.clone();
    let run_activity = activity.clone();
    let run_roster = roster.clone();
    let cap_roster = roster.clone();
    let abort_runner = runner.clone();
    let abort_log = log.clone();

    let client = ClientBuilder::new(backend_url.to_string())
        .auth(json!({ "token": jwt }))
        // (Re)advertise the current roster on connect.
        .on(Event::Connect, move |_payload, socket| {
            let roster = connect_roster.clone();
            async move {
                let payload = register_payload(&roster.lock().expect("roster lock"));
                let _ = socket.emit("medulla:register_agents", payload).await;
            }
            .boxed()
        })
        // A delegated task: relay it to the worker over tiny.place, reply up.
        //
        // CRITICAL: spawn rather than await here. A task can run for minutes, and
        // awaiting it inside the callback starves engine.io's ping/pong — the
        // server then drops us and every later delegation fails with "no harness
        // connected" while this process still looks alive.
        .on("medulla:task_run", move |payload, socket| {
            let run_log = run_log.clone();
            let runner = runner.clone();
            let roster = run_roster.clone();
            let run_activity = run_activity.clone();
            async move {
                tokio::spawn(handle_task_run(
                    payload,
                    socket,
                    runner,
                    roster,
                    run_log,
                    run_activity,
                ));
            }
            .boxed()
        })
        // Surface transport faults instead of dying silently.
        .on(Event::Error, {
            let log = log.clone();
            move |payload, _socket| {
                let log = log.clone();
                async move { log(&format!("hub: socket error: {payload:?}")) }.boxed()
            }
        })
        .on(Event::Close, {
            let log = log.clone();
            move |_payload, _socket| {
                let log = log.clone();
                async move { log("hub: socket closed — reconnecting") }.boxed()
            }
        })
        // The backend owns the task deadline and cancels a running task by
        // emitting `medulla:task_abort` ({ taskId }) — on its own deadline or an
        // explicit `/abort`. Relay it to the in-flight dispatch so it stops the
        // worker and reaps its correlation entry. This is the only path that
        // cancels a healthy worker still reporting progress; a silent or dead one
        // is caught by the runner's own liveness bounds. Cheap and non-blocking (a
        // registry lookup + `notify`), so it runs inline rather than spawned.
        .on("medulla:task_abort", move |payload, _socket| {
            let runner = abort_runner.clone();
            let log = abort_log.clone();
            async move {
                if let Some(task_id) = first_obj(payload).and_then(|o| str_field(&o, "taskId")) {
                    log(&format!("hub: task_abort {task_id} — stopping the worker"));
                    runner.abort_task(&task_id);
                }
            }
            .boxed()
        })
        // Capability probe: answer from the roster metadata.
        .on("medulla:capabilities_request", move |payload, socket| {
            let roster = cap_roster.clone();
            async move {
                handle_capabilities(payload, socket, roster).await;
            }
            .boxed()
        })
        // Survive transient drops; `Event::Connect` re-advertises the roster on
        // every reconnect, so the backend's view is restored automatically.
        .reconnect(true)
        .reconnect_on_disconnect(true)
        .reconnect_delay(1_000, 10_000)
        .connect()
        .await?;

    Ok(client)
}

/// Relay one `task_run` to its worker and emit the terminal `task_result`.
async fn handle_task_run(
    payload: Payload,
    socket: Client,
    runner: Arc<TaskRunner>,
    roster: SharedRoster,
    log: super::types::HubLog,
    activity: Option<super::ActivityLog>,
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
        provider: None,
        model: None,
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

    let frame = match outcome {
        Ok(outcome) => json!({
            "taskId": task_id,
            "ok": true,
            "reply": outcome.reply,
            "usage": {
                "inputTokens": outcome.usage.input_tokens,
                "outputTokens": outcome.usage.output_tokens,
            },
        }),
        Err(err) => {
            // Infra-shaped failures are retryable so medulla re-runs; a clean
            // worker error is terminal.
            let retryable = matches!(err, RunError::Timeout | RunError::Transport(_));
            json!({
                "taskId": task_id,
                "ok": false,
                "error": err.to_string(),
                "retryable": retryable,
            })
        }
    };
    let _ = socket.emit("medulla:task_result", frame).await;
}

/// Answer a capability probe from the roster metadata (static, so a probe never
/// blocks delegation).
async fn handle_capabilities(payload: Payload, socket: Client, roster: SharedRoster) {
    let Some(obj) = first_obj(payload) else {
        return;
    };
    let Some(probe_id) = str_field(&obj, "probeId") else {
        return;
    };
    let agent_id = str_field(&obj, "agentId").unwrap_or_default();
    // Extract what we need, then drop the lock before awaiting the emit.
    let (providers, summary) = {
        let r = roster.lock().expect("roster lock");
        match r.iter().find(|w| w.id == agent_id) {
            Some(w) => (vec![w.harness.clone()], format!("{} daemon", w.harness)),
            None => (Vec::new(), String::new()),
        }
    };
    let capabilities = json!({ "providers": providers, "summary": summary });
    let _ = socket
        .emit(
            "medulla:capabilities_result",
            json!({ "probeId": probe_id, "capabilities": capabilities }),
        )
        .await;
}
