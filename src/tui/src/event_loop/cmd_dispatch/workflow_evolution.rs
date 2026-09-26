//! Run workflow evolution turns on the TUI's tracked copilot host.

use std::collections::HashMap;
use std::path::PathBuf;

use medulla::daemon::embedded::EmbeddedDaemonOptions;
use medulla::workflows::LOCAL_WORKER_ADDRESS;

use super::AppMsg;

/// Spawn a workflow review and send its outcome to the workflow pane.
pub(super) fn spawn_evolve(
    workflow: String,
    run_id: Option<String>,
    workflows_config: medulla::config::WorkflowsConfig,
    launch: medulla::harness_hooks::LaunchPolicy,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        let (trigger, instruction) = match run_id {
            Some(run_id) => (
                medulla::workflows::evolve::EvolveTrigger::Failure(run_id.clone()),
                format!("Review this workflow, starting from run {run_id}."),
            ),
            None => (
                medulla::workflows::evolve::EvolveTrigger::Manual,
                "Review this workflow against its history.".to_string(),
            ),
        };
        let message = match evolve_turn(&workflow, trigger, &workflows_config, &launch).await {
            Ok(outcome) if outcome.skipped => AppMsg::CopilotDone {
                workflow,
                reply: "A review of this workflow is already running.".to_string(),
                changes: Vec::new(),
                created: None,
                removed: false,
            },
            Ok(outcome) => AppMsg::CopilotDone {
                workflow,
                reply: outcome.reply.clone(),
                changes: describe_review(&outcome),
                created: None,
                removed: false,
            },
            Err(error) => AppMsg::CopilotFailed {
                workflow,
                instruction,
                error: error.to_string(),
            },
        };
        let _ = tx.send(message);
        let _ = tx.send(AppMsg::WorkflowsChanged);
    });
}

/// Run an evolution turn using the cached ACP host for this workflow.
async fn evolve_turn(
    workflow: &str,
    trigger: medulla::workflows::evolve::EvolveTrigger,
    workflows_config: &medulla::config::WorkflowsConfig,
    launch: &medulla::harness_hooks::LaunchPolicy,
) -> anyhow::Result<medulla::workflows::evolve::EvolveOutcome> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    env.insert(
        medulla::daemon::providers::HARNESS_PROTOCOL_ENV.to_string(),
        "acp".to_string(),
    );
    medulla::mcp::preflight(&env, &cwd).map_err(anyhow::Error::msg)?;
    let store = medulla::workflows::discover_store(&env, &cwd);
    let (host, _) = super::copilot_hosts::host_for(workflow, || {
        EmbeddedDaemonOptions {
            workspace: cwd.to_string_lossy().to_string(),
            default_provider: workflows_config.default_provider,
            model: (!workflows_config.default_model.is_empty())
                .then(|| workflows_config.default_model.clone()),
            env,
            ..Default::default()
        }
        .with_launch_policy(launch)
    })
    .map_err(anyhow::Error::msg)?;
    let session = medulla::workflows::evolve::EvolveSession {
        store,
        dispatch: host.dispatch(),
        worker_address: LOCAL_WORKER_ADDRESS.to_string(),
        provider: workflows_config.default_provider,
        model: (!workflows_config.default_model.is_empty())
            .then(|| workflows_config.default_model.clone()),
        conversation: workflow.to_string(),
        config: medulla::workflows::evolve::EvolveConfig::from_config(workflows_config),
    };
    Ok(session.evolve(workflow, trigger, None).await?)
}

/// Format persisted review output for the workflow pane.
fn describe_review(outcome: &medulla::workflows::evolve::EvolveOutcome) -> Vec<String> {
    let mut lines = Vec::new();
    if !outcome.notes.is_empty() {
        lines.push(format!(
            "+ {} note{}",
            outcome.notes.len(),
            if outcome.notes.len() == 1 { "" } else { "s" }
        ));
    }
    for proposal in &outcome.proposals {
        lines.push(format!(
            "~ proposed: {}{}",
            proposal.rationale.trim(),
            if proposal.is_applicable() {
                ""
            } else {
                " (will not apply)"
            }
        ));
    }
    lines
}
