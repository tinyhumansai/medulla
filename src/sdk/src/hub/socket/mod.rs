//! The Socket.IO harness client — the hub's uplink to the hosted backend brain.
//!
//! Connects to the backend's harness plane, advertises the shared worker roster
//! (`medulla:register_agents`), and for every `medulla:task_run` the brain emits
//! it dispatches through the [`TaskRunner`] over tiny.place and streams the
//! result back (`medulla:task_result`, with `medulla:task_envelope` progress).
//! The roster is shared with the [`HubHandle`](super::HubHandle), so a worker
//! added at runtime is targetable and re-advertised immediately.
//!
//! The same connection carries the *workflow* plane: this host's saved graphs
//! are advertised beside the roster (`medulla:register_workflows`) and the reads
//! the orchestrator round-trips back (`medulla:workflow_request`) are served
//! from the installed bridge. See [`super::workflows`] for why every one of
//! those frames must be answered.
//!
//! This module owns the *connection* — the builder chain, the handler table, and
//! the small helpers every handler shares. Each down-event's body lives in its
//! own sibling ([`task_run`], [`capabilities`], [`workflow`]), so that serving a
//! frame can be tested without dialing a socket.

use futures::FutureExt;
use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Event, Payload, TransportType};
use serde_json::{json, Value};

use openhuman_core::openhuman::platform::socket::medulla::payloads::EVENT_WORKFLOW_REQUEST;

use super::roster::{addresses_of, register_payload, unreachable_addresses};

mod capabilities;
mod task_run;
mod types;
mod workflow;

pub(super) use types::HarnessWiring;

use capabilities::handle_capabilities;
use task_run::handle_task_run;
use workflow::{advertise_workflows, handle_workflow_request};

/// The retry policy, reachable from the hub's own suites.
///
/// Test-only because nothing in the connection path calls it — [`result_frame`]
/// applies it where the frame is built. Kept exported rather than duplicated in
/// the tests: a second copy of this rule would be free to disagree with the one
/// actually on the wire.
#[cfg(test)]
pub(in crate::hub) use task_run::{is_retryable, result_frame};

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
/// Authenticates with `jwt` in the Socket.IO handshake, advertises the roster
/// and (when one is installed) the workflow adverts on every (re)connect, and
/// dispatches `medulla:task_run` frames through the runner. The hub owns no task
/// deadline — the backend does — so nothing here bounds how long a task may run;
/// the runner only reaps a dispatch that goes silent. Returns the connected
/// client (which the [`HubHandle`](super::HubHandle) re-emits through); drop it
/// to disconnect.
pub(super) async fn connect_harness(
    backend_url: &str,
    jwt: &str,
    wiring: HarnessWiring,
) -> anyhow::Result<Client> {
    let HarnessWiring {
        roster,
        catalog,
        runner,
        subscription_strategy,
        log,
        activity,
        workflows: workflow_plane,
    } = wiring;
    let connect_roster = roster.clone();
    let connect_catalog = catalog.clone();
    let cap_catalog = catalog.clone();
    let connect_relay = runner.relay();
    let connect_log = log.clone();
    let connect_workflows = workflow_plane.clone();
    let request_workflows = workflow_plane;
    let request_log = log.clone();
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
        // (Re)advertise the current roster — and this host's workflows — on
        // connect.
        .on(Event::Connect, move |_payload, socket| {
            let roster = connect_roster.clone();
            let catalog = connect_catalog.clone();
            let relay = connect_relay.clone();
            let connect_log = connect_log.clone();
            let workflows = connect_workflows.clone();
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
                let payload =
                    { register_payload(&roster.lock().expect("roster lock"), &online, &catalog) };
                let _ = socket.emit("medulla:register_agents", payload).await;
                // Beside the roster, not instead of it: the backend keys a
                // workflow advert to the socket that sent it and drops the whole
                // entry on disconnect, so a reconnect that re-registered only
                // the agents would leave this host's graphs invisible until it
                // restarted.
                if let Some(workflows) = &workflows {
                    advertise_workflows(&socket, workflows, &connect_log).await;
                }
            }
            .boxed()
        })
        // A workflow round trip: read this host's store (or run one authoring
        // turn) and answer by `requestId`.
        //
        // Registered even when no bridge is installed, and spawned rather than
        // awaited: the backend holds a waiter for ten seconds on a read and ten
        // MINUTES on a copilot turn, so a request that goes unanswered is far
        // more expensive than one that is refused — and awaiting a copilot turn
        // inside the callback would starve engine.io's ping/pong exactly as a
        // long task would.
        .on(EVENT_WORKFLOW_REQUEST, move |payload, socket| {
            let bridge = request_workflows.clone();
            let log = request_log.clone();
            async move {
                tokio::spawn(handle_workflow_request(payload, socket, bridge, log));
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
            let catalog = cap_catalog.clone();
            let runner = cap_runner.clone();
            async move {
                tokio::spawn(handle_capabilities(
                    payload, socket, roster, catalog, runner,
                ));
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
