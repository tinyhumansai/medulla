//! Running, simulating, and editing a workflow from the TUI, off the render
//! thread.
//!
//! Each of these dispatches real harness sessions or touches the disk and can
//! take minutes, so none can happen inline: the app would stop repainting for
//! the whole of it. Every command is spawned, and the outcome comes back over
//! the [`AppMsg`] channel — the same shape every other long-running command
//! here uses.
//!
//! The copilot reports twice: progress lines while it works, then the reply and
//! the changes it made. Its pane is one turn's worth of surface, so a minute of
//! silence would read as a hang.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use medulla::daemon::embedded::EmbeddedDaemonOptions;
use medulla::flow_engine::{folding_sink, CapabilitySettings, HostServices};
use medulla::workflows::{
    run_workflow, LocalWorkflowHost, RunContext, StoreWorkflowResolver, LOCAL_WORKER_ADDRESS,
};

use super::AppMsg;

/// Spawn a run of the workflow `id`, reporting the outcome on the status line.
///
/// `workflows_config` is the `[workflows]` section the TUI already loaded at
/// startup (respecting `--config`, if one was passed) — carried in rather than
/// rediscovered, so a provider/model set only in an explicitly chosen config
/// file is honored here too rather than silently falling back to defaults.
pub(super) fn spawn_run(
    id: String,
    workflows_config: medulla::config::WorkflowsConfig,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        let status = match run(&id, &workflows_config).await {
            Ok(summary) => summary,
            Err(err) => format!("workflow '{id}' failed: {err}"),
        };
        let _ = tx.send(AppMsg::Status(status));
    });
}

/// Run the workflow to completion and describe how it ended.
async fn run(
    id: &str,
    workflows_config: &medulla::config::WorkflowsConfig,
) -> anyhow::Result<String> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let store = medulla::workflows::discover_store(&env, &cwd);

    let home = medulla::home::medulla_home(&env);
    let mut settings = CapabilitySettings::from_config(workflows_config, &home);
    if settings.default_worker_address.trim().is_empty() {
        settings.default_worker_address = LOCAL_WORKER_ADDRESS.to_string();
    }

    let host = LocalWorkflowHost::start(EmbeddedDaemonOptions {
        workspace: cwd.to_string_lossy().to_string(),
        default_provider: workflows_config.default_provider,
        model: (!workflows_config.default_model.is_empty())
            .then(|| workflows_config.default_model.clone()),
        ..Default::default()
    })
    .map_err(anyhow::Error::msg)?;

    // The host is held for the whole run and dropped with it, which unbinds the
    // loopback endpoints so a second run can bind them again.
    let (sink, _fold) = folding_sink();
    let run_id = format!("run-{}", uuid::Uuid::new_v4());
    let context = RunContext {
        store: store.clone(),
        settings: Arc::new(settings),
        services: HostServices {
            dispatch: host.dispatch(),
            resolver: Arc::new(StoreWorkflowResolver::new(store)),
            http_credentials: HashMap::new(),
        },
        sink,
    };

    let record = run_workflow(context, id, &run_id, serde_json::json!({})).await?;
    Ok(format!(
        "{id}: {} · {} step{}",
        medulla::ui::workflows::status_label(record.status),
        record.steps.len(),
        if record.steps.len() == 1 { "" } else { "s" }
    ))
}

/// What a copilot turn is being asked to do.
enum Turn {
    /// Change or explain the workflow with this id.
    Edit(String),
    /// Build a workflow that does not exist yet.
    Create,
}

/// Spawn a copilot turn against `workflow`, streaming its progress.
///
/// The turn runs on the same loopback host a workflow run uses, so the harness
/// it reaches is a real coding CLI on `PATH` with the `medulla-workflows` MCP
/// tools already attached to its session — the copilot's "tools to the graph"
/// are the operations every other authoring surface calls.
pub(super) fn spawn_copilot(
    workflow: String,
    instruction: String,
    workflows_config: medulla::config::WorkflowsConfig,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    spawn_turn(
        Turn::Edit(workflow.clone()),
        workflow,
        instruction,
        workflows_config,
        msg_tx,
    );
}

/// Spawn a copilot turn that builds a workflow from nothing.
///
/// The same session and the same tools as an edit; what differs is the prompt
/// (see [`medulla::workflows::copilot`]) and that the workflow it reports back
/// is discovered from the store rather than known in advance.
pub(super) fn spawn_copilot_create(
    thread: String,
    instruction: String,
    workflows_config: medulla::config::WorkflowsConfig,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    spawn_turn(Turn::Create, thread, instruction, workflows_config, msg_tx);
}

/// Run `turn` off-thread, forwarding its progress and reporting its result.
fn spawn_turn(
    turn: Turn,
    thread: String,
    instruction: String,
    workflows_config: medulla::config::WorkflowsConfig,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        // Progress lines are forwarded as they arrive rather than collected:
        // the point of them is that the pane is not silent while the turn runs.
        let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let forward = tokio::spawn({
            let tx = tx.clone();
            let thread = thread.clone();
            async move {
                while let Some(line) = status_rx.recv().await {
                    let _ = tx.send(AppMsg::CopilotStatus {
                        workflow: thread.clone(),
                        line,
                    });
                }
            }
        });

        let message =
            match copilot_turn(&turn, &thread, &instruction, status_tx, &workflows_config).await {
                Ok(outcome) => AppMsg::CopilotDone {
                    workflow: thread.clone(),
                    reply: outcome.reply,
                    changes: outcome.changes,
                    created: outcome.created,
                    removed: outcome.removed,
                },
                Err(err) => AppMsg::CopilotFailed {
                    workflow: thread.clone(),
                    error: err.to_string(),
                },
            };
        // The forwarder ends when the session drops its sender, which it has by
        // now; awaiting it keeps a trailing status line from arriving after the
        // reply and reading as part of the next turn.
        let _ = forward.await;
        let _ = tx.send(message);
    });
}

/// Run one copilot turn to completion.
///
/// The host is kept per thread rather than per turn (see [`copilot_hosts`]),
/// because the harness session a turn opens is remembered by the daemon — a
/// daemon that died with the turn would take the conversation with it.
/// `workflows_config` is the already-loaded `[workflows]` section (see
/// [`spawn_copilot`]) rather than a fresh [`medulla::config::load_config`] call
/// — reloading with no explicit path would silently drop a `--config` override.
async fn copilot_turn(
    turn: &Turn,
    thread: &str,
    instruction: &str,
    status: tokio::sync::mpsc::UnboundedSender<String>,
    workflows_config: &medulla::config::WorkflowsConfig,
) -> anyhow::Result<medulla::workflows::CopilotOutcome> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let store = medulla::workflows::discover_store(&env, &cwd);

    // ACP, not the legacy provider transport — this is the whole reason the
    // copilot can edit a graph at all. Medulla is an ACP *client*, so
    // `session/new`'s `mcpServers` is the only channel it has for handing a
    // harness the `medulla-workflows` tools; the legacy path has no equivalent
    // and leaves the agent with nothing but the filesystem, which the prompt
    // (rightly) forbids it from using. Forced here rather than left to the
    // operator's environment because a copilot without its tools is not a
    // degraded copilot, it is a chatbot that cannot do the one thing it is for.
    env.insert(
        medulla::daemon::providers::HARNESS_PROTOCOL_ENV.to_string(),
        "acp".to_string(),
    );

    // Asked before dispatching, not discovered from the result. Every way the
    // tools fail to arrive leaves a session that starts fine and can change
    // nothing — the operator would get a confident reply and an unchanged
    // graph, which is the one failure mode that looks like success.
    medulla::workflows::mcp::preflight(&env, &cwd).map_err(anyhow::Error::msg)?;

    let host = super::copilot_hosts::host_for(thread, || EmbeddedDaemonOptions {
        workspace: cwd.to_string_lossy().to_string(),
        default_provider: workflows_config.default_provider,
        model: (!workflows_config.default_model.is_empty())
            .then(|| workflows_config.default_model.clone()),
        env,
        ..Default::default()
    })
    .map_err(anyhow::Error::msg)?;

    let session = medulla::workflows::CopilotSession {
        store,
        dispatch: host.dispatch(),
        worker_address: LOCAL_WORKER_ADDRESS.to_string(),
        provider: workflows_config.default_provider,
        model: (!workflows_config.default_model.is_empty())
            .then(|| workflows_config.default_model.clone()),
        // The pane's thread is the conversation. Two workflows open side by side
        // are two threads and therefore two conversations, which is what the
        // operator means by having them open separately.
        conversation: thread.to_string(),
    };
    Ok(match turn {
        Turn::Edit(workflow) => session.turn(workflow, instruction, Some(status)).await?,
        Turn::Create => session.create(instruction, Some(status)).await?,
    })
}

/// Spawn an undo of the workflow `id`'s most recent edit.
///
/// Reloads the catalogue afterwards so the rail and the graph show the restored
/// version — an undo the operator cannot see has not visibly happened.
pub(super) fn spawn_undo(id: String, msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>) {
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        let env: HashMap<String, String> = std::env::vars().collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let store = medulla::workflows::discover_store(&env, &cwd);
        // Blocking work off the render thread: the store's methods are
        // synchronous by contract, as the trait's own doc says.
        let result = tokio::task::spawn_blocking(move || {
            medulla::workflows::undo_last(store.as_ref(), &id).map(|undone| (id, undone))
        })
        .await
        .unwrap_or_else(|err| Err(medulla::workflows::WorkflowError::Engine(err.to_string())));

        match result {
            Ok((id, Some((_, restored)))) => {
                // Name what came back rather than the revision's opaque id: the
                // operator is checking that undo landed where they meant, and
                // the workflow's own name is what they recognise.
                let _ = tx.send(AppMsg::Status(format!(
                    "Undid the last edit to {}",
                    restored.name
                )));
                let _ = tx.send(AppMsg::WorkflowsChanged);
                let _ = id;
            }
            Ok((id, None)) => {
                let _ = tx.send(AppMsg::Status(format!(
                    "{id} has not been edited since it was created — nothing to undo"
                )));
            }
            Err(err) => {
                let _ = tx.send(AppMsg::Status(format!("undo failed — {err}")));
            }
        }
    });
}

/// Spawn a dry run of the workflow `id`, reporting the outcome on the status
/// line.
///
/// A simulation resolves every expression and satisfies every declared output
/// shape without starting a harness session, so unlike a run it is safe to
/// press after an edit just to see whether the wiring holds.
pub(super) fn spawn_dry_run(id: String, msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>) {
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        let env: HashMap<String, String> = std::env::vars().collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let store = medulla::workflows::discover_store(&env, &cwd);
        let status =
            match medulla::workflows::ops::dry_run(&store, &id, serde_json::json!({})).await {
                Ok(_) => format!("{id}: simulation passed — every expression resolved"),
                Err(err) => format!("{id}: simulation failed — {err}"),
            };
        let _ = tx.send(AppMsg::Status(status));
    });
}
