//! Running one delegated task as a thread on a shared `codex app-server`.
//!
//! The shape mirrors the CLI seam exactly — same [`RunTaskOptions`] in, same
//! [`RunTaskResult`] out — so everything upstream of the transport is unchanged.
//! What differs is that no process is spawned here: a thread is opened on a
//! pooled one, and closing the lane leaves the process running for the next.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::codex_app_server::{
    pool, AppServerError, AppServerSpec, Connection, ThreadOptions, ThreadSubscription,
    TurnOutcome, TurnStatus,
};
use crate::protocol::{HarnessProvider, HarnessTransport};

use super::super::types::{OnEvent, RunTaskOptions, RunTaskResult};
use super::fold::FoldState;

/// Environment switch selecting the app-server transport, parallel to the ACP
/// one.
///
/// The transport is normally chosen by naming the `codex-server` harness, which
/// arrives on the task frame. This exists for the same reason the ACP switch
/// does: a wrapper or a locally-launched harness has no frame to state it on.
pub const HARNESS_TRANSPORT_ENV: &str = "MEDULLA_HARNESS_TRANSPORT";

/// Whether this task should run over the shared app-server.
///
/// The frame's transport wins; the environment switch is the fallback for
/// callers that have no frame.
pub(in crate::daemon::providers) fn uses_app_server(options: &RunTaskOptions) -> bool {
    if options.transport == HarnessTransport::AppServer {
        return true;
    }
    options
        .env
        .get(HARNESS_TRANSPORT_ENV)
        .and_then(|value| HarnessTransport::from_wire(value.trim()))
        .is_some_and(|transport| transport == HarnessTransport::AppServer)
}

/// Execute one task as a thread on a pooled `codex app-server`.
pub async fn run_codex_server_task(options: RunTaskOptions) -> Result<RunTaskResult, String> {
    if options.provider != HarnessProvider::Codex {
        // Refused rather than downgraded: an operator who asked for the shared
        // process and silently got a CLI fork has no way to notice.
        return Err(format!(
            "the app-server transport is Codex-only; `{}` cannot run it",
            options.provider.as_str()
        ));
    }
    run(options).await.map_err(|error| error.to_string())
}

/// The body, in terms of the app-server's own error type.
async fn run(options: RunTaskOptions) -> Result<RunTaskResult, AppServerError> {
    let spec = AppServerSpec {
        bin: crate::protocol::env::provider_bin(HarnessProvider::Codex, &options.env),
        args: Vec::new(),
        env: child_env(&options),
    };
    let connection = pool().acquire(&spec).await?;

    let mut thread = ThreadOptions::from_permissions(
        options.cwd.clone(),
        options.model.clone(),
        options.skip_permissions,
    );
    thread.resume_thread_id = options
        .resume_session_id
        .clone()
        .filter(|id| !id.trim().is_empty());

    let thread_id = open_thread(&connection, &thread).await?;
    // Subscribed before the turn starts, so a turn cannot outrun its own
    // listener — see `Connection::subscribe`.
    let mut subscription = connection.subscribe(&thread_id, options.skip_permissions);

    if let Some(callback) = options.on_session {
        callback(thread_id.clone());
    }

    let outcome = drive_turn(
        &connection,
        &mut subscription,
        &thread_id,
        thread.cwd,
        options.prompt,
        options.model,
        options.timeout_ms,
        options.abort,
        options.on_event,
        options.workspace_context,
        options.on_workspace_context,
    )
    .await?;

    if let Some(message) = outcome.error {
        return Err(AppServerError::Rpc {
            method: "turn/start".to_string(),
            message,
        });
    }
    Ok(RunTaskResult {
        provider: HarnessProvider::Codex,
        reply: outcome.reply,
        events: outcome.items,
        usage: outcome.usage,
        session_id: Some(outcome.thread_id),
    })
}

/// The environment the pooled process runs with.
///
/// The same two layers the ACP seam applies, and for the same reasons: the child
/// is what runs `git commit`, so attribution has to reach it, and a configured
/// `[router]` points at Medulla's loopback proxy, which an unrouted child would
/// walk straight past.
///
/// A fleet grant is stripped. Unlike the CLI transport, this environment belongs
/// to a process *shared by several tasks*, so an ambient capability here would
/// be redeemable by every lane on it, not just the one it was minted for.
pub(super) fn child_env(options: &RunTaskOptions) -> HashMap<String, String> {
    let mut env = options.env.clone();
    crate::protocol::env::scrub_core_state(&mut env, options.provider);
    env.remove(crate::control_socket::MCP_SOCKET_ENV);
    env.remove(crate::control_socket::MCP_GRANT_ENV);
    env.remove(crate::control_socket::MCP_PARENT_SOCKET_ENV);
    env.remove(crate::control_socket::MCP_PARENT_GRANT_ENV);
    let attribution_env = crate::attribution::attribution_env(options.attribution, &env);
    env.extend(attribution_env);
    if let Some(router) = &options.router {
        let injection = crate::protocol::env::router_env(HarnessProvider::Codex, router);
        for (key, value) in injection.env {
            env.insert(key, value);
        }
        for (child_var, source_name) in injection.secret_env {
            if let Some(secret) = options.env.get(&source_name).filter(|v| !v.is_empty()) {
                env.insert(child_var, secret.clone());
            }
        }
    }
    env
}

/// Start or resume the thread this task runs on, returning its id.
///
/// A resume the server refuses falls back to a fresh thread rather than failing
/// the task: the id may name a thread from a previous Codex install or a
/// rolled-off history, and losing continuity is a smaller harm than losing the
/// run.
async fn open_thread(
    connection: &Connection,
    thread: &ThreadOptions,
) -> Result<String, AppServerError> {
    if let Some(resume) = &thread.resume_thread_id {
        let mut params = thread_params(thread);
        params["threadId"] = json!(resume);
        match connection.call("thread/resume", params).await {
            Ok(result) => return thread_id_from(&result),
            Err(error) => tracing::info!(
                target: "medulla::codex_app_server",
                thread = %resume,
                %error,
                "codex app-server declined a resume; starting a fresh thread"
            ),
        }
    }
    let result = connection
        .call("thread/start", thread_params(thread))
        .await?;
    thread_id_from(&result)
}

/// The parameters `thread/start` and `thread/resume` share.
fn thread_params(thread: &ThreadOptions) -> Value {
    json!({
        "cwd": thread.cwd,
        "sandbox": thread.sandbox,
        "approvalPolicy": thread.approval_policy,
        "model": thread.model,
    })
}

/// Pull the thread id out of a `thread/start` or `thread/resume` result.
fn thread_id_from(result: &Value) -> Result<String, AppServerError> {
    result
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppServerError::Protocol("thread result carried no id".into()))
}

/// Send the prompt and read notifications until the turn is terminal.
///
/// Three things end the wait besides completion, and each has to leave the
/// *thread* in a defined state rather than merely returning: an abort, the idle
/// watchdog, and the connection dying. The first two interrupt the turn, so the
/// shared process is not left working on output nobody will read — which matters
/// far more here than on the CLI transport, where killing the child ended the
/// work by construction.
///
/// `turn/start` is a request, and the server answers it when the turn settles.
/// It is nonetheless the *notification* that ends this loop, because the
/// response says only that the turn is over while `turn/completed` says how. The
/// response is still awaited alongside, so a turn the server refuses outright
/// fails with its own message instead of waiting out the idle watchdog.
#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    connection: &Connection,
    subscription: &mut ThreadSubscription,
    thread_id: &str,
    cwd: String,
    prompt: String,
    model: Option<String>,
    timeout_ms: u64,
    abort: super::super::types::Abort,
    on_event: Option<OnEvent>,
    workspace_context: crate::sessions::WorkspaceContext,
    on_workspace_context: Option<super::super::types::OnWorkspaceContext>,
) -> Result<TurnOutcome, AppServerError> {
    // Shared because the idle watchdog reads the fold's last-activity stamp
    // while the notification branch writes to it, and both live in one
    // `select!`.
    let fold = Arc::new(Mutex::new(FoldState::with_workspace(
        on_event,
        workspace_context,
        on_workspace_context,
    )));
    let timeout = Duration::from_millis(timeout_ms);
    // Tracked from `turn/started` so an interrupt can name the turn it stops;
    // the protocol requires both ids.
    let mut turn_id: Option<String> = None;

    let request = connection.call(
        "turn/start",
        json!({
            "threadId": thread_id,
            // Codex treats cwd as sticky turn state. Send it even after
            // `thread/resume`: otherwise a retained worktree exists only in
            // Medulla's metadata while built-in tools default to the original
            // directory recorded by the thread.
            "cwd": cwd,
            "input": [{ "type": "text", "text": prompt }],
            "model": model,
        }),
    );
    tokio::pin!(request);
    let mut request_settled = false;

    let status = loop {
        let idle = idle_after(fold.clone(), timeout);
        tokio::select! {
            result = &mut request, if !request_settled => {
                request_settled = true;
                // Only a refusal is terminal here. A success means the turn is
                // over, and the loop goes back to reading so the
                // `turn/completed` already in flight is folded rather than
                // dropped.
                result?;
            }
            notification = subscription.recv() => match notification {
                Some(notification) => {
                    if let Some(id) = started_turn_id(&notification) {
                        turn_id = Some(id);
                    }
                    let terminal = notification.method == "turn/completed";
                    let status = terminal.then(|| turn_status(&notification.params));
                    fold.lock().expect("codex app-server fold lock").fold(&notification);
                    if let Some(status) = status {
                        break status;
                    }
                }
                // The subscription only closes when the connection dies.
                None => return Err(AppServerError::Process(
                    "the codex app-server exited while the turn was running".into(),
                )),
            },
            _ = abort.cancelled() => {
                interrupt(connection, thread_id, turn_id.as_deref());
                return Err(AppServerError::Aborted);
            }
            _ = idle => {
                interrupt(connection, thread_id, turn_id.as_deref());
                return Err(AppServerError::Idle { timeout_ms });
            }
        }
    };

    let fold = fold.lock().expect("codex app-server fold lock").snapshot();
    Ok(TurnOutcome {
        thread_id: thread_id.to_string(),
        reply: fold.reply,
        usage: fold.usage,
        error: match status {
            TurnStatus::Failed => Some(
                fold.error
                    .unwrap_or_else(|| "the turn failed with no message".to_string()),
            ),
            _ => None,
        },
        items: fold.items,
    })
}

/// Resolve once the fold has been silent for `timeout`.
///
/// The deadline is re-read each pass rather than computed once, because every
/// notification pushes it out — the same "activity resets the watchdog" rule the
/// CLI and ACP transports use.
async fn idle_after(fold: Arc<Mutex<FoldState>>, timeout: Duration) {
    loop {
        let last_activity = fold
            .lock()
            .expect("codex app-server fold lock")
            .last_activity;
        tokio::time::sleep_until((last_activity + timeout).into()).await;
        let last_activity = fold
            .lock()
            .expect("codex app-server fold lock")
            .last_activity;
        if std::time::Instant::now().duration_since(last_activity) >= timeout {
            return;
        }
    }
}

/// The turn id a `turn/started` notification announces.
fn started_turn_id(notification: &crate::codex_app_server::Notification) -> Option<String> {
    if notification.method != "turn/started" {
        return None;
    }
    notification
        .params
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The terminal status a `turn/completed` notification reports.
fn turn_status(params: &Value) -> TurnStatus {
    params
        .get("turn")
        .and_then(|turn| turn.get("status"))
        .and_then(Value::as_str)
        .map(TurnStatus::from_wire)
        .unwrap_or(TurnStatus::Completed)
}

/// Ask the server to stop a turn, best-effort.
///
/// Failures are ignored deliberately: this runs on paths that are already
/// returning an error, and a connection too broken to accept an interrupt has
/// stopped the turn by other means.
fn interrupt(connection: &Connection, thread_id: &str, turn_id: Option<&str>) {
    let Some(turn_id) = turn_id else {
        return;
    };
    let _ = connection.call_detached(
        "turn/interrupt",
        json!({ "threadId": thread_id, "turnId": turn_id }),
    );
}
