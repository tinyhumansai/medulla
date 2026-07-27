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
pub(super) fn spawn_run(id: String, msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>) {
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        let status = match run(&id).await {
            Ok(summary) => summary,
            Err(err) => format!("workflow '{id}' failed: {err}"),
        };
        let _ = tx.send(AppMsg::Status(status));
    });
}

/// Run the workflow to completion and describe how it ended.
async fn run(id: &str) -> anyhow::Result<String> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let store = medulla::workflows::discover_store(&env, &cwd);
    let loaded = medulla::config::load_config(None, &env, &cwd)?;

    let home = medulla::home::medulla_home(&env);
    let mut settings = CapabilitySettings::from_config(&loaded.config.workflows, &home);
    if settings.default_worker_address.trim().is_empty() {
        settings.default_worker_address = LOCAL_WORKER_ADDRESS.to_string();
    }

    let host = LocalWorkflowHost::start(EmbeddedDaemonOptions {
        workspace: cwd.to_string_lossy().to_string(),
        default_provider: loaded.config.workflows.default_provider,
        model: (!loaded.config.workflows.default_model.is_empty())
            .then(|| loaded.config.workflows.default_model.clone()),
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

/// Spawn a copilot turn against `workflow`, streaming its progress.
///
/// The turn runs on the same loopback host a workflow run uses, so the harness
/// it reaches is a real coding CLI on `PATH` with the `medulla-workflows` MCP
/// tools already attached to its session — the copilot's "tools to the graph"
/// are the operations every other authoring surface calls.
pub(super) fn spawn_copilot(
    workflow: String,
    instruction: String,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        // Progress lines are forwarded as they arrive rather than collected:
        // the point of them is that the pane is not silent while the turn runs.
        let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let forward = tokio::spawn({
            let tx = tx.clone();
            let workflow = workflow.clone();
            async move {
                while let Some(line) = status_rx.recv().await {
                    let _ = tx.send(AppMsg::CopilotStatus {
                        workflow: workflow.clone(),
                        line,
                    });
                }
            }
        });

        let message = match copilot_turn(&workflow, &instruction, status_tx).await {
            Ok(outcome) => AppMsg::CopilotDone {
                workflow: workflow.clone(),
                reply: outcome.reply,
                changes: outcome.changes,
            },
            Err(err) => AppMsg::CopilotFailed {
                workflow: workflow.clone(),
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
async fn copilot_turn(
    workflow: &str,
    instruction: &str,
    status: tokio::sync::mpsc::UnboundedSender<String>,
) -> anyhow::Result<medulla::workflows::CopilotOutcome> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let store = medulla::workflows::discover_store(&env, &cwd);
    let loaded = medulla::config::load_config(None, &env, &cwd)?;

    let host = LocalWorkflowHost::start(EmbeddedDaemonOptions {
        workspace: cwd.to_string_lossy().to_string(),
        default_provider: loaded.config.workflows.default_provider,
        model: (!loaded.config.workflows.default_model.is_empty())
            .then(|| loaded.config.workflows.default_model.clone()),
        ..Default::default()
    })
    .map_err(anyhow::Error::msg)?;

    let session = medulla::workflows::CopilotSession {
        store,
        dispatch: host.dispatch(),
        worker_address: LOCAL_WORKER_ADDRESS.to_string(),
        provider: loaded.config.workflows.default_provider,
        model: (!loaded.config.workflows.default_model.is_empty())
            .then(|| loaded.config.workflows.default_model.clone()),
    };
    Ok(session.turn(workflow, instruction, Some(status)).await?)
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
