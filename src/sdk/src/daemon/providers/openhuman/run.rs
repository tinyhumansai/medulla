//! Executing one task as an in-process OpenHuman agent turn.
//!
//! Two things beyond the prompt make that turn able to do real work, and both
//! are scoped around the dispatch rather than passed as parameters — see
//! [`run_openhuman_task`] and the task-locals it enters.

use std::path::PathBuf;
use std::time::Duration;

use openhuman_core::openhuman::agent::turn_origin::{
    with_origin, AgentTurnOrigin, TrustedAutomationSource,
};
use openhuman_core::openhuman::agent::turn_workspace::with_workspace;
use serde_json::{json, Value};

use crate::protocol::{HarnessEvent, HarnessProvider};

use super::super::types::{RunTaskOptions, RunTaskOrigin, RunTaskResult};

/// The core method that runs a full agent turn.
///
/// The tool-using loop with memory and the approval gate, not
/// `inference_agent_chat_simple` beside it — that one is a bare model
/// completion, which would make an OpenHuman node a strictly worse `llm` node
/// rather than a harness.
const AGENT_CHAT: &str = "openhuman.inference_agent_chat";

/// Whether `options` should run on the embedded core rather than a spawned CLI.
///
/// A function rather than an inline `matches!` at the one call site, for the
/// same reason [`super::super::acp::uses_acp`] is one: the transport decisions
/// in [`super::super::execute::run_provider_task`] read as a list of questions,
/// and one of them phrased differently is one a reader has to stop at.
pub fn uses_embedded_core(options: &RunTaskOptions) -> bool {
    options.provider == HarnessProvider::Openhuman
}

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
///   [`TrustedAutomationSource::Workflow`], which is what this scopes.
/// * **Workspace.** The run names a checkout ([`RunTaskOptions::cwd`]). Scoping
///   it makes it both the turn's working directory and a read/write root for
///   the path policy, so a write into that tree is not refused as an escape
///   from the core's own `workspace_dir`. See
///   [`openhuman_core::openhuman::agent::turn_workspace`] on why the grant is
///   no stronger than a configured trusted root.
///
/// # Errors
///
/// Returns a sentence when the core cannot be started, when the turn is
/// aborted or exceeds `timeout_ms`, or when the core itself refuses the call —
/// the same failure vocabulary a spawned provider returns, so a caller does not
/// branch on which harness ran.
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
        mut on_event,
        on_session,
        ..
    } = options;

    // The operator's environment override outranks whatever the dispatch
    // resolved; see [`super::model`] for the whole precedence order.
    let model = super::effective_model(model, &env);

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

    // Synthesized rather than folded from a stream: there is no transcript to
    // tail, so the prompt and the reply are the only two things this turn can
    // honestly report. Emitting them keeps an OpenHuman step's recorded
    // transcript the same shape as every other harness's, which is what lets
    // the run view render one without knowing which provider ran it.
    emit(&mut on_event, "user_prompt", json!({ "text": prompt }));

    // snake_case, not camelCase: `AgentChatParams` carries no `rename_all`, so
    // the controller deserializes the field names as they are spelled in Rust.
    let params = json!({
        "message": prompt,
        "model_override": model,
        "thread_id": thread_id,
    });
    // `AgentChatParams` carries no origin or workspace field; those trust
    // decisions instead ride task-locals through the full core dispatch. The
    // Medulla-owned cwd scope reaches the process-global lifecycle hooks too,
    // so a `PostToolUse` auto-commit targets this run's checkout.
    let cwd_path = PathBuf::from(&cwd);
    let call = crate::core_host::turn_cwd::with_turn_cwd(
        Some(cwd_path.as_path()),
        scoped_workspace(
            &cwd,
            scoped_origin(origin, &thread_id, core.raw().invoke(AGENT_CHAT, params)),
        ),
    );

    // The same idle ceiling a spawned provider gets, applied to the whole turn
    // rather than to the gap between events: an in-process call produces no
    // events to reset a watchdog with, so "idle" and "running" are the same
    // observation here. `timeout_ms` of 0 means the operator set no ceiling.
    let outcome = if timeout_ms == 0 {
        tokio::select! {
            result = call => result,
            _ = abort.cancelled() => return Err("openhuman task aborted".to_string()),
        }
    } else {
        tokio::select! {
            result = call => result,
            _ = abort.cancelled() => return Err("openhuman task aborted".to_string()),
            _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {
                return Err(format!("openhuman task idle for {timeout_ms}ms (no events)"));
            }
        }
    };

    let reply = match outcome {
        Ok(value) => reply_text(value),
        Err(err) => {
            emit(
                &mut on_event,
                "error",
                json!({ "message": err, "fatal": true }),
            );
            return Err(format!("openhuman turn failed: {err}"));
        }
    };
    let reply = if reply.trim().is_empty() {
        "OpenHuman completed without a text response.".to_string()
    } else {
        reply
    };
    emit(&mut on_event, "agent_message", json!({ "text": reply }));

    Ok(RunTaskResult {
        provider: HarnessProvider::Openhuman,
        reply,
        // One prompt in, one answer out. Counted honestly rather than reported
        // as zero: a caller reading `events == 0` concludes nothing happened.
        events: 2,
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

/// Run `fut` with the run's checkout scoped as the turn's workspace.
///
/// A no-op when `cwd` does not resolve to a directory — see
/// [`turn_workspace_root`]. Written as a wrapper rather than an `if` at the
/// call site because the two arms have different types: entering a task-local
/// scope changes the future, and only a function can hide that.
async fn scoped_workspace<F: std::future::Future>(cwd: &str, fut: F) -> F::Output {
    match turn_workspace_root(cwd) {
        Some(root) => with_workspace(root, fut).await,
        None => fut.await,
    }
}

/// The absolute directory `cwd` names, when it names one.
///
/// Returns `None` for the empty string and for anything that is not a
/// directory on this machine. Both are ordinary rather than exceptional: a
/// dispatch that never set a working directory arrives with `"."` or `""`, and
/// a stale path is a host's mistake that should leave the turn on the core's
/// own workspace rather than granting a root that does not exist.
///
/// Canonicalized because the grant is a `starts_with` containment check on the
/// paths the tools resolve: a symlinked or `..`-laden root would fail to
/// contain its own contents and quietly refuse every write into it.
pub(super) fn turn_workspace_root(cwd: &str) -> Option<PathBuf> {
    if cwd.is_empty() {
        return None;
    }
    let resolved = std::fs::canonicalize(cwd).ok()?;
    if !resolved.is_dir() {
        tracing::warn!(
            cwd = %resolved.display(),
            "openhuman turn: the run's working directory is not a directory — \
             the turn stays on the core's own workspace",
        );
        return None;
    }
    Some(resolved)
}

/// Hand one synthesized event to the caller's callback, when there is one.
fn emit(on_event: &mut Option<super::super::types::OnEvent>, kind: &str, payload: Value) {
    let Some(callback) = on_event.as_mut() else {
        return;
    };
    callback(&crate::daemon::mappers::HarnessSemanticEvent {
        line: 0,
        timestamp_ms: crate::clock::now_millis(),
        record_type: format!("openhuman:{kind}"),
        event: HarnessEvent {
            kind: kind.to_string(),
            payload,
            ..Default::default()
        },
    });
}

/// The answer text out of whatever shape the controller returned.
///
/// The core serializes through `RpcOutcome`, whose wire shape is *variable*: a
/// handler that logged nothing returns the value itself, and one that logged
/// anything returns `{ "result": …, "logs": [...] }`. Whether a given method
/// logs is an implementation detail that can change without notice, so both are
/// accepted rather than the one this method happens to emit today.
pub(super) fn reply_text(value: Value) -> String {
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
