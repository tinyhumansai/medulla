//! The Socket.IO harness client — the hub's uplink to the hosted backend brain.
//!
//! Connects to the backend's harness plane, advertises the shared worker roster
//! (`medulla:register_agents`), and for every `medulla:task_run` the brain emits
//! it dispatches through the [`TaskRunner`] over tiny.place and streams the
//! result back (`medulla:task_result`, with `medulla:task_envelope` progress).
//! The roster is shared with the [`HubHandle`](super::HubHandle), so a worker
//! added at runtime is targetable and re-advertised immediately.

use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Event, Payload};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::roster::{address_of, register_payload, SharedRoster};

/// Margin subtracted from the backend's own per-task deadline so the hub replies
/// with a real error just before the backend times out blind.
const BACKEND_MARGIN: Duration = Duration::from_secs(5);

use super::runner::TaskRunner;
use super::types::{RunError, TaskRequest};

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
/// every (re)connect, and dispatches `medulla:task_run` frames through `runner`
/// with a `task_timeout` deadline. Returns the connected client (which the
/// [`HubHandle`](super::HubHandle) re-emits through); drop it to disconnect.
pub async fn connect_harness(
    backend_url: &str,
    jwt: &str,
    roster: SharedRoster,
    runner: Arc<TaskRunner>,
    task_timeout: Duration,
) -> anyhow::Result<Client> {
    let connect_roster = roster.clone();
    let run_roster = roster.clone();
    let cap_roster = roster.clone();

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
            let runner = runner.clone();
            let roster = run_roster.clone();
            async move {
                tokio::spawn(handle_task_run(
                    payload,
                    socket,
                    runner,
                    roster,
                    task_timeout,
                ));
            }
            .boxed()
        })
        // Surface transport faults instead of dying silently.
        .on(Event::Error, |payload, _socket| {
            async move { eprintln!("hub: socket error: {payload:?}") }.boxed()
        })
        .on(Event::Close, |_payload, _socket| {
            async move { eprintln!("hub: socket closed — reconnecting") }.boxed()
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
    timeout: Duration,
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
    // The backend puts ITS OWN deadline on the frame (`timeoutMs`, its
    // `DEFAULT_TASK_TIMEOUT_MS` clamped by `MAX_TASK_TIMEOUT_MS`). Honor it minus
    // a margin so the hub always reports a real error just BEFORE the backend
    // gives up with a blind "subagent task timeout" — and never exceed the
    // locally-configured cap. Falls back to the cap when the field is absent.
    let timeout = obj
        .get("timeoutMs")
        .and_then(|v| v.as_u64())
        .map(Duration::from_millis)
        .map(|d| d.saturating_sub(BACKEND_MARGIN).min(timeout))
        .filter(|d| !d.is_zero())
        .unwrap_or(timeout);

    // Resolve the address, then drop the lock before any await (the std guard is
    // not held across suspension points). An empty roster ⇒ nothing to run.
    let worker_address = {
        let r = roster.lock().expect("roster lock");
        address_of(&r, &agent_id)
    };
    let Some(worker_address) = worker_address else {
        let _ = socket
            .emit(
                "medulla:task_result",
                json!({ "taskId": task_id, "ok": false, "error": "hub has no workers", "retryable": false }),
            )
            .await;
        return;
    };

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

    eprintln!(
        "hub: task_run {} → {} (timeout {}s)",
        task_id,
        worker_address,
        timeout.as_secs()
    );

    let req = TaskRequest {
        task_id: task_id.clone(),
        cycle_id,
        instruction,
        worker_address,
        provider: None,
        model: None,
        timeout,
    };

    let outcome = runner.run(req, Some(tx)).await;
    match &outcome {
        Ok(o) => eprintln!("hub: task {} ok ({} chars)", task_id, o.reply.len()),
        Err(e) => eprintln!("hub: task {} FAILED: {e}", task_id),
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
