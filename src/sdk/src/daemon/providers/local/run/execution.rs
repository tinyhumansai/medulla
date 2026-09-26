//! The one task-shaped entry point: resolving the route, driving the turn under
//! the watchdog, and reporting what it produced.
//!
//! Kept apart from [`super`]'s module wiring so the routing decision there
//! ([`super::uses_local_harness`]) reads separately from what a routed turn
//! actually does. Everything else in [`super`] is this file's support: the
//! supervision that keeps the turn alive ([`super::watchdog`]), the fold that
//! turns its event stream into Medulla's vocabulary ([`super::progress`]), and
//! the sink that emits the result ([`super::events`]).

use std::sync::Arc;

use serde_json::json;

use crate::agent::harness::{Limits, Route};
use crate::agent::tools::Workspace;
use crate::protocol::HarnessProvider;

use super::super::super::types::{RunTaskOptions, RunTaskResult};
use super::watchdog;
use super::EventSink;

/// Slack, in harness events, between the turn and this supervisor.
///
/// The forwarder is synchronous and must not block the agent loop, so it uses
/// `try_send` and this bound is what it may run ahead by. `ModelDelta` arrives
/// roughly per token, so the burst to absorb is a streamed paragraph, not a tool
/// call — a few hundred events. 512 covers that while capping the queue at a few
/// hundred small values, and the watchdog drains continuously rather than in
/// batches, so the steady state sits near empty.
const PROGRESS_CAPACITY: usize = 512;

/// Run one task as a local agent turn in this process.
///
/// # What the turn is allowed to do
///
/// The run names a checkout ([`RunTaskOptions::cwd`]) and the turn's tools are
/// rooted there: the filesystem tools refuse a path that resolves outside it,
/// and the shell tool runs with it as the working directory. The shell is *not*
/// sandboxed — see [`crate::agent::tools::shell`], which states the boundary it
/// does and does not provide rather than leaving a reader to assume one.
///
/// # Errors
///
/// Returns a sentence when the route cannot be resolved, when the turn is
/// aborted or falls silent for `timeout_ms`, or when the harness itself refuses
/// the call — the same failure vocabulary a spawned provider returns, so a
/// caller does not branch on which harness ran.
pub async fn run_local_task(options: RunTaskOptions) -> Result<RunTaskResult, String> {
    let RunTaskOptions {
        prompt,
        cwd,
        model,
        env,
        timeout_ms,
        abort,
        resume_session_id,
        router,
        on_event,
        on_session,
        ..
    } = options;

    // The operator's environment override outranks whatever the dispatch
    // resolved; see [`super::super::model`] for the whole precedence order.
    let model = super::super::effective_model(model, &env);

    // A preset's endpoint and key, resolved into the mount the turn should call
    // and the token it should present.
    let route = super::super::embedded_route(router.as_ref(), &env, model.as_deref())?;

    if abort.is_aborted() {
        return Err("local task aborted before start".to_string());
    }

    // Unlike the embedded core, which resolved its own provider bindings from
    // ambient config when a preset named none, this harness has nothing to fall
    // back to: it needs an endpoint and a credential to make any call at all.
    // Saying so is better than a turn that fails on its first request with a
    // provider error about a missing key.
    let route = route.ok_or_else(|| {
        "the local harness needs an inference route: name a router preset on the node, \
         or set the preset's key variable in the environment"
            .to_string()
    })?;
    let route = Route {
        base_url: route.base_url,
        token: route.token,
        // `effective_model` answers `None` when nothing named one, which is a
        // real state for a CLI provider (the child picks its own default) but
        // not for this one — the request carries a model id or the endpoint
        // rejects it.
        model: model.ok_or_else(|| {
            "the local harness needs a model: name one on the node, or set the \
             preset's model variable in the environment"
                .to_string()
        })?,
    };

    // The turn's continuity key. A bounded workflow node arrives with no resume
    // id and gets a fresh thread — which is the isolation a node needs, and the
    // same thing `--session-id` buys on the CLI providers.
    let thread_id =
        resume_session_id.unwrap_or_else(|| format!("medulla-{}", uuid::Uuid::new_v4()));
    if let Some(callback) = on_session {
        callback(thread_id.clone());
    }

    let mut sink = EventSink::new(on_event);

    // Synthesized rather than folded from the stream: the prompt reaches the
    // transcript only as part of the final message list, and a first line that
    // arrives last is not one an operator can watch. Emitting it here keeps a
    // local step's transcript the same shape as every other harness's, which is
    // what lets the run view render one without knowing which provider ran it.
    sink.emit("user_prompt", json!({ "text": prompt }));

    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::channel::<tinyagents::events::AgentEvent>(PROGRESS_CAPACITY);
    let workspace = Workspace {
        root: std::path::PathBuf::from(&cwd),
    };

    // `try_send`, not `send`: the listener the harness calls this from is
    // synchronous, so there is nowhere to await. A full queue drops the event
    // rather than blocking the agent loop — the transcript is best-effort and
    // the turn is not.
    let forward = Arc::new(move |event| {
        let _ = progress_tx.try_send(event);
    });

    let limits = Limits::default();
    // The task's curated environment, the same map a spawned CLI harness gets —
    // never this process's own. The early return in `execute::run_provider_task`
    // skips the child-env preparation the spawn seam does, so handing the tools
    // an inherited environment would put `MEDULLA_TOKEN` and every router key
    // one `env` command away from the model.
    // The account home this process resolves, which is where the thread's
    // transcript lives. Read from the task's own environment rather than the
    // process's, so a run pointed at a scratch `MEDULLA_HOME` keeps its threads
    // there too.
    let home = crate::home::medulla_home(&env);
    let call = crate::agent::turn::run(
        crate::agent::turn::TurnRequest {
            prompt: &prompt,
            route: &route,
            workspace,
            limits: &limits,
            thread_id: &thread_id,
            home: &home,
            env,
        },
        forward,
    );

    let outcome = watchdog::drive(call, &mut progress_rx, &abort, timeout_ms, &mut sink).await?;

    let turn = match outcome {
        Ok(turn) => turn,
        Err(err) => {
            sink.emit("error", json!({ "message": err, "fatal": true }));
            return Err(format!("local turn failed: {err}"));
        }
    };

    let reply = if turn.reply.trim().is_empty() {
        "The agent completed without a text response.".to_string()
    } else {
        turn.reply
    };
    sink.emit("agent_message", json!({ "text": reply }));

    Ok(RunTaskResult {
        provider: HarnessProvider::Openhuman,
        reply,
        // What the turn actually produced: the synthesized prompt and reply
        // plus every folded harness event, counted as they were emitted.
        events: sink.emitted(),
        // Reported when the provider gave a count. Unlike the embedded core,
        // which billed its own turns and exposed no per-call figure, the route
        // here is an ordinary chat-completions endpoint and the harness folds
        // whatever usage it returns.
        usage: turn
            .usage
            .filter(|(input, output)| *input > 0 || *output > 0)
            .map(|(input, output)| crate::protocol::TokenUsage {
                input_tokens: input as i64,
                output_tokens: output as i64,
            }),
        session_id: Some(thread_id),
    })
}
