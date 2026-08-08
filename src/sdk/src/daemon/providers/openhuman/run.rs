//! Executing one task as an in-process OpenHuman agent turn.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::protocol::{HarnessEvent, HarnessProvider};

use super::super::types::{RunTaskOptions, RunTaskResult};

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
/// # Errors
///
/// Returns a sentence when the core cannot be started, when the turn is
/// aborted or exceeds `timeout_ms`, or when the core itself refuses the call —
/// the same failure vocabulary a spawned provider returns, so a caller does not
/// branch on which harness ran.
pub async fn run_openhuman_task(options: RunTaskOptions) -> Result<RunTaskResult, String> {
    let RunTaskOptions {
        prompt,
        model,
        timeout_ms,
        abort,
        resume_session_id,
        hooks,
        mut on_event,
        on_session,
        ..
    } = options;

    // Said once, at the top, rather than left for an operator to infer from an
    // empty hook log. There is no child process here, so there is no argv for
    // `harness_hooks` to install onto and nothing for a hook to observe.
    let configured = hooks.for_provider(HarnessProvider::Openhuman).len();
    if configured > 0 {
        tracing::warn!(
            hooks = configured,
            "medulla hooks are not installed for OpenHuman: the turn runs in this process, \
             so there is no child harness for a lifecycle hook to wrap",
        );
    }

    if abort.is_aborted() {
        return Err("openhuman task aborted before start".to_string());
    }

    let core = crate::core_host::shared::shared().await?;

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
    let call = core.raw().invoke(AGENT_CHAT, params);

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
