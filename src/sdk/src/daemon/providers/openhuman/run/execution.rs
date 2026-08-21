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

use openhuman_core::embed::Route;
use openhuman_core::openhuman::agent::turn_origin::{AgentTurnOrigin, TrustedAutomationSource};
use serde_json::json;

use crate::protocol::HarnessProvider;

use super::super::super::types::{RunTaskOptions, RunTaskOrigin, RunTaskResult};
use super::core_contract::{AgentProgress, ProgressSink};
use super::watchdog;
use super::EventSink;

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
    let harness = crate::core_host::shared::shared_with_hooks(&hooks).await?;

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

    // Annotated with the core's own alias rather than inferred: the sender half
    // *is* the contract's `ProgressSink`, and saying so keeps a future change to
    // its element type a compile error here instead of a silent mismatch.
    let (progress_tx, mut progress_rx): (ProgressSink, _) =
        tokio::sync::mpsc::channel::<AgentProgress>(PROGRESS_CAPACITY);
    let cwd_path = std::path::PathBuf::from(&cwd);

    // The turn, described rather than hand-encoded. This used to be a
    // `serde_json::json!` literal whose keys had to match `AgentChatParams`
    // field-for-field — with no `rename_all` upstream, an unmarked rename there
    // was a silent runtime failure here. The builder owns that contract now, and
    // the progress sink and the origin scope ride with it instead of being
    // task-locals this file has to remember to enter.
    let mut turn = harness
        .turn(&prompt)
        // The core's own continuity key, minted above because the caller is
        // told the session id before the turn runs.
        .session(&thread_id)
        // Roots the turn's file and shell tools in the node's checkout, so
        // relative paths resolve there rather than in the Medulla process's
        // startup directory.
        .cwd(&cwd)
        .on_progress(progress_tx);

    if let Some(model) = model.as_deref() {
        turn = turn.model(model);
    }
    // Only when the preset asked for it. `Route` takes the endpoint and the key
    // together because the core ignores a half of the pair — an absent route is
    // what "run on the account's own inference" looks like.
    if let Some(route) = route {
        turn = turn.route(Route::openai_compatible(route.base_url, route.token));
    }
    // Unattended workflow authority, for workflow nodes only. Delegated tasks,
    // conversational turns, local sessions and capability probes stay
    // unlabelled on purpose: OpenHuman then applies its fail-closed approval
    // policy to their external-effect tools.
    if origin == RunTaskOrigin::Workflow {
        turn = turn.origin(AgentTurnOrigin::TrustedAutomation {
            // The turn's own id, so an audit row or a parked approval names the
            // dispatch it came from rather than a constant.
            job_id: thread_id.clone(),
            source: TrustedAutomationSource::Workflow {
                // The node already ran because the operator's graph said it
                // should; parking each tool call for a second decision would
                // strand an unattended run on a prompt nobody is watching.
                require_approval: false,
            },
        });
    }

    // `with_turn_cwd` is still scoped by hand: it is read by Medulla's own
    // process-global lifecycle hooks while a tool runs, not by the core. Without
    // it a `PostToolUse` auto-commit hook is told the Medulla process's startup
    // directory and checkpoints the wrong repository. See
    // `core_host::turn_cwd`.
    let call = crate::core_host::turn_cwd::with_turn_cwd(Some(cwd_path.as_path()), turn.send());

    let outcome = watchdog::drive(call, &mut progress_rx, &abort, timeout_ms, &mut sink).await?;

    let reply = match outcome {
        Ok(outcome) => outcome.reply,
        Err(err) => {
            // Rendered here rather than carried structurally: every other
            // harness reports a failure as a sentence, and a caller that had to
            // branch on which one ran would have to know which harness it got.
            // `CoreError`'s `Display` already names the method and the domain
            // message, which is what an operator reads.
            let message = err.to_string();
            sink.emit("error", json!({ "message": message, "fatal": true }));
            return Err(format!("openhuman turn failed: {message}"));
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
