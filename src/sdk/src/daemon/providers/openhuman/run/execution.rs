//! The one task-shaped entry point: booting the core, calling its agent-chat
//! method under the watchdog, and decoding the reply.
//!
//! Kept apart from [`super`]'s module wiring so the routing decisions there
//! ([`super::uses_embedded_core`]) read separately from what a routed turn
//! actually does. Everything else in [`super`] is this file's support: the
//! supervision that keeps the turn alive ([`super::watchdog`]), and the sink
//! that turns its progress stream into events ([`super::events`]).
//!
//! Two things beyond the prompt make that turn able to do real work, and both
//! are scoped around the dispatch rather than passed as parameters — see
//! [`run_openhuman_task`] and the task-locals it enters.

use openhuman_core::openhuman::agent::turn_origin::{
    with_origin, AgentTurnOrigin, TrustedAutomationSource,
};
use serde_json::{json, Value};

use crate::protocol::HarnessProvider;

use super::super::super::types::{RunTaskOptions, RunTaskOrigin, RunTaskResult};
use super::core_contract::{with_progress_sink, AgentProgress, ProgressSink};
use super::watchdog;
use super::EventSink;

/// The core method that runs a full agent turn.
///
/// The tool-using loop with memory and the approval gate, not
/// `inference_agent_chat_simple` beside it — that one is a bare model
/// completion, which would make an OpenHuman node a strictly worse `llm` node
/// rather than a harness.
const AGENT_CHAT: &str = "openhuman.inference_agent_chat";

/// Slack, in progress events, between the core and this supervisor.
///
/// The core *awaits* its sends, so this bound is backpressure on the turn
/// itself: too small and a chatty turn stalls the model between tokens waiting
/// for the watchdog to be scheduled. `TextDelta` arrives roughly per token, so
/// the burst to absorb is a streamed paragraph, not a tool call — a few hundred
/// events. 256 covers that while capping the queue at a few hundred small enum
/// values, and the loop drains continuously rather than in batches, so the
/// steady state sits near empty.
const PROGRESS_CAPACITY: usize = 256;

/// Run one task as an OpenHuman agent turn in this process.
///
/// # What the turn is allowed to do
///
/// Per-turn state is scoped around the dispatch, and without the OpenHuman
/// task-locals the turn cannot do the work a node asks of it:
///
/// * **Origin.** OpenHuman's approval gate refuses every external-effect tool
///   (`shell`, `edit`, `apply_patch`, the `*_exec` family) from a call site
///   that carries no [`AgentTurnOrigin`] — the fail-closed default for an
///   unlabelled caller. A workflow node is not unlabelled: the graph that runs
///   it was authored and saved by the operator, so its actions carry the same
///   trust root a user-authored cron job's do. That is exactly
///   [`TrustedAutomationSource::Workflow`], which is what this scopes. See
///   [`scoped_origin`].
/// * **Workspace.** The run names a checkout ([`RunTaskOptions::cwd`]):
///   `AgentChatParams::cwd` roots the turn's file and shell tools there, so
///   relative paths resolve inside the node's checkout rather than the Medulla
///   process's startup directory, and [`crate::core_host::turn_cwd`] scopes the
///   same tree for the process-global lifecycle hooks.
///
/// # Errors
///
/// Returns a sentence when the core cannot be started, when the turn is
/// aborted or falls silent for `timeout_ms`, or when the core itself refuses
/// the call — the same failure vocabulary a spawned provider returns, so a
/// caller does not branch on which harness ran.
pub async fn run_openhuman_task(options: RunTaskOptions) -> Result<RunTaskResult, String> {
    let RunTaskOptions {
        prompt,
        origin,
        cwd,
        model,
        env,
        timeout_ms,
        abort,
        resume_session_id,
        hooks,
        router,
        on_event,
        on_session,
        ..
    } = options;

    // The operator's environment override outranks whatever the dispatch
    // resolved; see [`super::super::model`] for the whole precedence order.
    let model = super::super::effective_model(model, &env);

    // A preset's endpoint and key, resolved into the loopback mount the core
    // should call and the token it should present. `None` for a run with no
    // OpenRouter-bound router or no key under the named variable, which leaves
    // the core resolving its own provider bindings exactly as before.
    let route = super::super::openrouter_route(router.as_ref(), &env, model.as_deref())?;

    if abort.is_aborted() {
        return Err("openhuman task aborted before start".to_string());
    }

    // A headless workflow may be the first OpenHuman caller in this process.
    // Its hooks must reach that lazy boot; an already installed TUI core is
    // retained by `shared_with_hooks` and already owns its hook registration.
    let core = crate::core_host::shared::shared_with_hooks(&hooks).await?;

    // The core's own continuity key. A bounded workflow node arrives with no
    // resume id and gets a fresh thread — which is the isolation a node needs,
    // and the same thing `--session-id` buys on the CLI providers.
    let thread_id =
        resume_session_id.unwrap_or_else(|| format!("medulla-{}", uuid::Uuid::new_v4()));
    if let Some(callback) = on_session {
        callback(thread_id.clone());
    }

    let mut sink = EventSink::new(on_event);

    // Synthesized rather than folded from the progress stream: the core reports
    // the prompt back only as part of `TurnContent`, at the end, and a
    // transcript whose first line arrives last is not one an operator can watch.
    // Emitting it here keeps an OpenHuman step's transcript the same shape as
    // every other harness's, which is what lets the run view render one without
    // knowing which provider ran it.
    sink.emit("user_prompt", json!({ "text": prompt }));

    // snake_case, not camelCase: `AgentChatParams` carries no `rename_all`, so
    // the controller deserializes the field names as they are spelled in Rust.
    // `cwd` is optional on the core side and roots the turn's file and shell
    // tools in the node's checkout; the origin trust decision rides a
    // task-local instead — see the scope below.
    let mut params = json!({
        "message": prompt,
        "model_override": model,
        "thread_id": thread_id,
        "cwd": &cwd,
    });
    // Only when the preset asked for it. The core treats the pair as one
    // statement and ignores a half of it, so both keys are added together or
    // neither is — an absent pair is what "run on the account's own inference"
    // looks like on the wire.
    if let Some(route) = route {
        let map = params
            .as_object_mut()
            .expect("the params literal above is an object");
        map.insert("inference_url".into(), json!(route.base_url));
        map.insert("api_key".into(), json!(route.token));
    }

    // Annotated with the core's own alias rather than inferred: the sender half
    // *is* the contract's `ProgressSink`, and saying so keeps a future change to
    // its element type a compile error here instead of a silent mismatch.
    let (progress_tx, mut progress_rx): (ProgressSink, _) =
        tokio::sync::mpsc::channel::<AgentProgress>(PROGRESS_CAPACITY);
    let cwd_path = std::path::PathBuf::from(&cwd);

    // Task-local scopes around the same call, composed by wrapping, in the
    // order the supervisor's own scopes read shortest-lived-first:
    //
    // * `with_progress_sink` is read by the core when it builds this turn's
    //   agent, and is what makes the watchdog below an idle watchdog rather
    //   than a stopwatch.
    // * `with_turn_cwd` is read by the process-global lifecycle hooks while a
    //   tool runs — without it a `PostToolUse` auto-commit hook is told the
    //   Medulla process's startup directory and checkpoints the wrong
    //   repository. See `core_host::turn_cwd`.
    // * `scoped_origin` labels a workflow node's external-effect tools as
    //   operator-authorized automation; every other dispatch stays unlabelled
    //   and OpenHuman's approval gate fails closed for it.
    let call = with_progress_sink(
        progress_tx,
        crate::core_host::turn_cwd::with_turn_cwd(
            Some(cwd_path.as_path()),
            scoped_origin(origin, &thread_id, core.raw().invoke(AGENT_CHAT, params)),
        ),
    );

    let outcome = watchdog::drive(call, &mut progress_rx, &abort, timeout_ms, &mut sink).await?;

    let reply = match outcome {
        Ok(value) => reply_text(value),
        Err(err) => {
            sink.emit("error", json!({ "message": err, "fatal": true }));
            return Err(format!("openhuman turn failed: {err}"));
        }
    };
    let reply = if reply.trim().is_empty() {
        "OpenHuman completed without a text response.".to_string()
    } else {
        reply
    };
    sink.emit("agent_message", json!({ "text": reply }));

    Ok(RunTaskResult {
        provider: HarnessProvider::Openhuman,
        reply,
        // What the turn actually produced: the synthesized prompt and reply
        // plus every folded progress event, counted as they were emitted.
        events: sink.emitted(),
        // The core bills its own turns against the operator's account and does
        // not report per-call token counts through this method, so claiming a
        // number here would invent one.
        usage: None,
        session_id: Some(thread_id),
    })
}

/// Run `future` with unattended workflow authority only for workflow nodes.
///
/// Delegated tasks, conversational turns, local sessions, and capability
/// probes intentionally remain unlabelled: OpenHuman then applies its
/// fail-closed approval policy to their external-effect tools.
async fn scoped_origin<F>(origin: RunTaskOrigin, thread_id: &str, future: F) -> F::Output
where
    F: std::future::Future,
{
    if origin == RunTaskOrigin::Workflow {
        with_origin(
            AgentTurnOrigin::TrustedAutomation {
                // The turn's own id, so an audit row or a parked approval names
                // the dispatch it came from rather than a constant.
                job_id: thread_id.to_string(),
                source: TrustedAutomationSource::Workflow {
                    // The node already ran because the operator's graph said it
                    // should; parking each tool call for a second decision would
                    // strand an unattended run on a prompt nobody is watching.
                    require_approval: false,
                },
            },
            future,
        )
        .await
    } else {
        future.await
    }
}

/// The answer text out of whatever shape the controller returned.
///
/// The core serializes through `RpcOutcome`, whose wire shape is *variable*: a
/// handler that logged nothing returns the value itself, and one that logged
/// anything returns `{ "result": …, "logs": [...] }`. Whether a given method
/// logs is an implementation detail that can change without notice, so both are
/// accepted rather than the one this method happens to emit today.
pub(in crate::daemon::providers) fn reply_text(value: Value) -> String {
    let payload = value
        .get("result")
        .filter(|_| value.get("logs").is_some_and(Value::is_array))
        .unwrap_or(&value);
    match payload {
        Value::String(text) => text.clone(),
        // Not expected from this method, but a controller that one day answers
        // structurally should be rendered rather than reported as empty.
        other => other.to_string(),
    }
}
