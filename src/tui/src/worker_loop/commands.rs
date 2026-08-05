//! Async effects emitted by the daemon TUI.

use medulla::bridge::Bridge as _;
use medulla::daemon::DaemonRuntime;
use medulla::protocol::HarnessProvider;
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
        WorkerCmd::Start { mode, provider } => {
            start_worker(app, start, inbox, runtime, mode, provider)
        }
        WorkerCmd::ConnectMaster(input) => connect_master(app, start, input).await,
        WorkerCmd::MessageMaster { address, text } => {
            message_master(app, start, address, text).await
        }
        WorkerCmd::AddWorkspace(input) => add_workspace(app, runtime, input).await,
        WorkerCmd::RemoveWorkspace(workspace) => remove_workspace(app, runtime, workspace).await,
    }
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
        app.set_status("No host-link identity — this worker serves local sessions only");
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
    daemon.enable_screen_kill();
    // Screens are only worth streaming when there are screens: the headless
    // executor runs harnesses without a pty, so a subscriber finds nothing.
    let screens =
        medulla_tui::worker::stream::ScreenRouter::new(start.sessions.clone(), daemon.clone(), {
            let sender = transport.clone();
            medulla_tui::worker::stream::send_fn(move |to: String, body: String| {
                let sender = sender.clone();
                async move {
                    let _ = sender.send(&to, &body).await;
                }
            })
        })
        .with_log(start.logs.sink());
    *inbox = Some(spawn_inbox_drain(transport, daemon.clone(), screens));
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
    // No handshake to perform: enrollment already established the pair key, so
    // adding the row is the whole operation (`docs/host-link-protocol.md` §7).
    app.add_master(address.clone(), handle);
    if let Err(err) = medulla::config::persist_link_peers(app.config_path(), app.masters()) {
        app.set_status(format!(
            "Master {address} added, but config was not saved: {err}"
        ));
    } else {
        app.set_status(format!("Master {address} added"));
    }
}

/// Send an encrypted operator message to an accepted master.
async fn message_master(app: &mut WorkerApp, start: &StartWiring, address: String, text: String) {
    let Some(transport) = start.transport.clone() else {
        app.set_status("Worker identity unavailable — cannot message the master");
        return;
    };
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
