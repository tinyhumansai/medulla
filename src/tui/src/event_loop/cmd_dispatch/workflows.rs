//! Running a workflow from the TUI, off the render thread.
//!
//! A workflow run dispatches real harness sessions and takes minutes, so it
//! cannot happen inline: the app would stop repainting for the whole run. The
//! command is spawned, and the status line carries the outcome — which is the
//! same shape every other long-running command here uses.

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
