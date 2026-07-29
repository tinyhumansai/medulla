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
use rust_socketio::{Event, Payload, TransportType};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::roster::{
    address_of, addresses_of, register_payload, unreachable_addresses, SharedRoster,
    SharedSubscriptionStrategy,
};
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
    subscription_strategy: SharedSubscriptionStrategy,
    log: super::types::HubLog,
    activity: Option<super::ActivityLog>,
) -> anyhow::Result<Client> {
    let connect_roster = roster.clone();
    let connect_relay = runner.relay();
    let connect_log = log.clone();
    let run_log = log.clone();
    let run_activity = activity.clone();
    let run_roster = roster.clone();
    let cap_roster = roster.clone();
    let cap_runner = runner.clone();
    let abort_runner = runner.clone();
    let abort_log = log.clone();
    let run_subscription_strategy = subscription_strategy.clone();

    let client = ClientBuilder::new(backend_url.to_string())
        .auth(json!({ "token": jwt }))
        // Websocket only — never engine.io's polling handshake.
        //
        // That handshake mints a session id on ONE server process and requires
        // every following poll to reach the same one. Behind a load balancer
        // fronting several replicas that only holds if the client returns the
        // balancer's affinity cookie, and `rust_engineio` implements no cookie
        // handling at all: it sends none, so each poll is routed afresh and the
        // server answers `{"code":1,"message":"Session ID unknown"}`. Observed
        // against production, where the hub failed to connect roughly half the
        // time and was dropped seconds later when it did — leaving the agent
        // roster unregistered and the orchestrator reporting no hosts at all.
        //
        // A websocket is one connection: it is established once and stays on
        // the process that accepted it, so no affinity is required. The cost is
        // that a network which blocks websockets can no longer fall back to
        // polling — an acceptable trade, since polling cannot work here anyway.
        .transport_type(TransportType::Websocket)
        // (Re)advertise the current roster on connect.
        .on(Event::Connect, move |_payload, socket| {
            let roster = connect_roster.clone();
            let relay = connect_relay.clone();
            let connect_log = connect_log.clone();
            async move {
                // Liveness first, so the opening advertisement is already
                // truthful. Registering optimistically and correcting later
                // leaves a window in which the orchestrator can pick a worker
                // that is not there — which is the whole failure being fixed.
                let addresses = { addresses_of(&roster.lock().expect("roster lock")) };
                let online = relay.presence(&addresses).await;
                // Said out loud, because a roster that quietly shrinks is
                // indistinguishable from one that was never configured — and an
                // agent advertised without this line has two causes that look
                // identical: the relay reported it up, or the relay said nothing
                // and the optimistic default applied.
                let withheld = {
                    let r = roster.lock().expect("roster lock");
                    unreachable_addresses(&r, &online)
                };
                (connect_log)(&format!(
                    "hub: presence for {} worker(s) — {}{}",
                    addresses.len(),
                    if online.is_empty() {
                        "no answer, advertising all".to_string()
                    } else {
                        addresses
                            .iter()
                            .map(|a| match online.get(a) {
                                Some(true) => format!("{a} online"),
                                Some(false) => format!("{a} OFFLINE"),
                                None => format!("{a} unknown"),
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                    if withheld.is_empty() {
                        String::new()
                    } else {
                        format!(" — withholding {} from agent_list", withheld.len())
                    }
                ));
                let payload = { register_payload(&roster.lock().expect("roster lock"), &online) };
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
            let subscription_strategy = run_subscription_strategy.clone();
            async move {
                tokio::spawn(handle_task_run(
                    payload,
                    socket,
                    runner,
                    roster,
                    subscription_strategy,
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
        // Capability probe: answer from the roster facts, decorated with the
        // worker's live budgets/readiness probed over tiny.place.
        //
        // CRITICAL: spawn rather than await, for the same reason as `task_run`.
        // The live probe hops to the worker over tiny.place and can take tens of
        // seconds (its harness readiness probe); awaiting it inside the callback
        // starves engine.io's ping/pong and the server drops the hub, failing
        // every later delegation while this process still looks alive.
        .on("medulla:capabilities_request", move |payload, socket| {
            let roster = cap_roster.clone();
            let runner = cap_runner.clone();
            async move {
                tokio::spawn(handle_capabilities(payload, socket, roster, runner));
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
pub(super) fn is_retryable(err: &RunError) -> bool {
    matches!(
        err,
        RunError::Timeout | RunError::Transport(_) | RunError::Busy(_)
    )
}

async fn handle_task_run(
    payload: Payload,
    socket: Client,
    runner: Arc<TaskRunner>,
    roster: SharedRoster,
    subscription_strategy: SharedSubscriptionStrategy,
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
    let requested_provider = str_field(&obj, "provider")
        .as_deref()
        .and_then(crate::tinyplace::HarnessProvider::from_wire);
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
                        super::roster::subscription_for_strategy(&capabilities, strategy)
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
        model,
        // Forwarded rather than dropped: a worker advertises the workflows it
        // has installed, so the orchestrator naming one here is the other half
        // of that conversation. Blank is treated as absent so an emitter that
        // always writes the key still dispatches an ordinary instruction.
        workflow: obj
            .get("workflow")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string),
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
            let retryable = is_retryable(&err);
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

/// Answer a capability probe, decorating the static roster facts with the
/// worker's live budgets/readiness.
///
/// The static facts (`providers`, `summary`) are established from the roster
/// without touching the worker, so a probe always answers even if the worker is
/// unreachable. On top of that, the hub asks the resolved worker for its
/// [`AgentCapabilities`] over tiny.place and maps its `budgets`/`readiness` onto
/// the backend-shaped keys (`harnessBudgets`, `ready`, `readyReason`) that the
/// backend's `sanitizeCapabilities` reads. The probe fails open: any transport
/// error, timeout, or malformed reply simply omits those keys rather than
/// blocking the answer.
async fn handle_capabilities(
    payload: Payload,
    socket: Client,
    roster: SharedRoster,
    runner: Arc<TaskRunner>,
) {
    let Some(obj) = first_obj(payload) else {
        return;
    };
    let Some(probe_id) = str_field(&obj, "probeId") else {
        return;
    };
    let agent_id = str_field(&obj, "agentId").unwrap_or_default();
    // Resolve the targeted worker (or the selected/first when unattributed),
    // then drop the lock before any await.
    let worker = {
        let r = roster.lock().expect("roster lock");
        let wanted = agent_id.trim();
        let found = if wanted.is_empty() {
            r.iter().find(|w| w.selected).or_else(|| r.first())
        } else {
            r.iter().find(|w| w.id == wanted || w.address == wanted)
        };
        found.cloned()
    };
    let (harness, address) = match &worker {
        Some(w) => (w.harness.clone(), Some(w.address.clone())),
        None => (String::new(), None),
    };
    // Ask the worker what it can actually do, and answer with that. Fails open:
    // a transport error, timeout, or malformed reply leaves only the static
    // facts the roster already knows. See [`super::probe`].
    let caps = match address {
        Some(address) => runner.capabilities(&address).await.ok(),
        None => None,
    };
    let capabilities = super::probe::capabilities_payload(&harness, caps.as_ref());
    let _ = socket
        .emit(
            "medulla:capabilities_result",
            json!({ "probeId": probe_id, "capabilities": capabilities }),
        )
        .await;
}
