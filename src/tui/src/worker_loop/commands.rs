//! Async effects emitted by the daemon TUI.

use medulla::daemon::DaemonRuntime;
use medulla::tinyplace::HarnessProvider;
use medulla_tui::worker::app::{ExecutionMode, WorkerApp, WorkerCmd};

use super::{claude_preflight, spawn_inbox_drain, worker_runtime, StartWiring};

/// Execute one command the screen emitted.
pub(super) async fn run_cmd(
    app: &mut WorkerApp,
    cmd: WorkerCmd,
    start: &StartWiring,
    inbox: &mut Option<tokio::task::JoinHandle<()>>,
    runtime: &mut Option<DaemonRuntime>,
) {
    match cmd {
        WorkerCmd::Quit => app.should_quit = true,
        WorkerCmd::Refresh => refresh(app).await,
        WorkerCmd::Start { mode, provider } => {
            start_worker(app, start, inbox, runtime, mode, provider)
        }
        WorkerCmd::ContactOp { agent_id, decision } => {
            let Some(desk) = app.contact_desk() else {
                app.set_status("No tiny.place identity — contact decisions are unavailable");
                return;
            };
            let status = match desk.decide(&agent_id, decision).await {
                Ok(status) => status,
                Err(message) => message,
            };
            app.set_status(status);
        }
        WorkerCmd::ConnectMaster(input) => connect_master(app, start, input).await,
        WorkerCmd::MessageMaster { address, text } => {
            message_master(app, start, address, text).await
        }
        WorkerCmd::AddWorkspace(input) => add_workspace(app, runtime, input).await,
        WorkerCmd::RemoveWorkspace(workspace) => remove_workspace(app, runtime, workspace).await,
    }
}

/// Force a relay/contact refresh.
async fn refresh(app: &mut WorkerApp) {
    let Some(desk) = app.contact_desk() else {
        app.set_status("No tiny.place identity — nothing to refresh");
        return;
    };
    let health = desk.refresh().await;
    app.set_status(match &health {
        medulla::contacts::PollHealth::Failed { error, .. } => {
            format!("Relay unreachable: {error}")
        }
        _ => format!(
            "Relay checked · {} request(s) known, {} pending",
            desk.requests().len(),
            desk.pending_count()
        ),
    });
}

/// Build the selected executor and begin draining encrypted work.
fn start_worker(
    app: &mut WorkerApp,
    start: &StartWiring,
    inbox: &mut Option<tokio::task::JoinHandle<()>>,
    runtime: &mut Option<DaemonRuntime>,
    mode: ExecutionMode,
    provider: HarnessProvider,
) {
    let Some(transport) = start.transport.clone() else {
        app.set_status("No tiny.place identity — this worker serves local sessions only");
        return;
    };
    if inbox.is_some() {
        return;
    }
    if mode == ExecutionMode::Interactive && provider == HarnessProvider::Claude {
        claude_preflight(start, app);
    }
    start.logs.push(if start.skip_permissions {
        format!(
            "{}: permission checks bypassed for peer tasks (--no-skip-permissions declines)",
            provider.as_str()
        )
    } else {
        format!(
            "{}: permission checks left on — a task that stops to ask will hang",
            provider.as_str()
        )
    });
    let daemon = worker_runtime(start, mode, provider, &transport);
    *inbox = Some(spawn_inbox_drain(transport, daemon.clone()));
    *runtime = Some(daemon);
    app.set_status(format!(
        "Serving peers · {} on {}",
        mode.as_str(),
        provider.as_str()
    ));
}

/// Resolve, request, and persist a master identity.
async fn connect_master(app: &mut WorkerApp, start: &StartWiring, input: String) {
    let Some(transport) = start.transport.clone() else {
        app.set_status("Worker identity unavailable — cannot connect a master");
        return;
    };
    let handle = input
        .trim()
        .starts_with('@')
        .then(|| input.trim().to_string());
    let address = if handle.is_some() {
        match transport.resolve_handle(&input).await {
            Some(address) => address,
            None => {
                app.set_status(format!("Master {input} was not found"));
                return;
            }
        }
    } else {
        input.trim().to_string()
    };
    match transport.request_contact(&address).await {
        Ok(()) => {
            app.add_master(address.clone(), handle);
            if let Err(err) =
                medulla::config::persist_tinyplace_peers(app.config_path(), app.masters())
            {
                app.set_status(format!(
                    "Contact requested, but config was not saved: {err}"
                ));
            } else {
                app.set_status(format!(
                    "Master {address} added · contact requested, awaiting approval"
                ));
            }
        }
        Err(err) => app.set_status(format!("Could not connect master: {err}")),
    }
}

/// Send an encrypted operator message to an accepted master.
async fn message_master(app: &mut WorkerApp, start: &StartWiring, address: String, text: String) {
    let Some(transport) = start.transport.clone() else {
        app.set_status("Worker identity unavailable — cannot message the master");
        return;
    };
    if !transport.contact_accepted(&address).await {
        app.set_status("Master has not accepted this worker's contact request yet");
        return;
    }
    match transport.send(&address, &text).await {
        Ok(()) => app.set_status(format!("Sent to master · {} chars", text.chars().count())),
        Err(err) => app.set_status(format!("Message failed: {err}")),
    }
}

/// Validate and advertise one workspace root.
async fn add_workspace(app: &mut WorkerApp, runtime: &mut Option<DaemonRuntime>, input: String) {
    let path = std::path::PathBuf::from(input.trim());
    let canonical = match std::fs::canonicalize(&path) {
        Ok(path) if path.is_dir() => path.to_string_lossy().into_owned(),
        Ok(_) => {
            app.set_status("Workspace must be a directory");
            return;
        }
        Err(err) => {
            app.set_status(format!("Workspace is unavailable: {err}"));
            return;
        }
    };
    app.add_workspace(canonical.clone());
    if let Err(err) =
        medulla::config::persist_workflow_workspaces(app.config_path(), app.workspaces())
    {
        app.set_status(format!("Workspace was not saved: {err}"));
        return;
    }
    if let Some(runtime) = runtime.as_ref() {
        runtime.set_accessible_dirs(app.workspaces().to_vec()).await;
    }
    app.set_status(format!("Allowed and advertised {canonical}"));
}

/// Remove one non-primary workspace from capability advertisement.
async fn remove_workspace(
    app: &mut WorkerApp,
    runtime: &mut Option<DaemonRuntime>,
    workspace: String,
) {
    if !app.remove_workspace(&workspace) {
        return;
    }
    if let Err(err) =
        medulla::config::persist_workflow_workspaces(app.config_path(), app.workspaces())
    {
        app.set_status(format!("Workspace removal was not saved: {err}"));
        return;
    }
    if let Some(runtime) = runtime.as_ref() {
        runtime.set_accessible_dirs(app.workspaces().to_vec()).await;
    }
    app.set_status(format!("Stopped advertising {workspace}"));
}
