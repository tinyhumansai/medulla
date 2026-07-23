//! Blocking local-Git and GitHub CLI jobs spawned by the interactive loop.

use std::path::PathBuf;

use super::types::AppMsg;
use medulla_tui::ui::app::{App, Cmd};

/// Fold fresh local reports, then launch their selected diff and Ship probes.
pub(super) fn apply_workspaces(
    app: &mut App,
    reports: Vec<medulla::workspace::WorkspaceReport>,
    tx: tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    app.set_workspace_reports(reports);
    app.set_status("Repo · refreshed");
    if let Some(Cmd::LoadWorkspaceDiff { workspace, path }) = app.selected_repo_diff_cmd() {
        load_diff(workspace, path, tx.clone());
    }
    app.set_ship_loading();
    load_ship(app.loaded.workflow_workspaces(), tx);
}

/// Fold fresh Ship rows, then load the selected PR's failure excerpt.
pub(super) fn apply_ship(
    app: &mut App,
    reports: Vec<medulla::ship::WorkspaceShipReport>,
    tx: tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    app.set_ship_reports(reports);
    if let Some(Cmd::LoadShipLog { workspace, number }) = app.selected_ship_log_cmd() {
        load_ship_log(workspace, number, tx);
    }
}

/// Inspect configured repositories without blocking terminal redraw.
pub(super) fn load_workspaces(roots: Vec<PathBuf>, tx: tokio::sync::mpsc::UnboundedSender<AppMsg>) {
    tokio::task::spawn_blocking(move || {
        let reports = roots
            .into_iter()
            .map(|root| {
                let result = medulla::workspace::inspect_workspace(&root);
                medulla::workspace::WorkspaceReport::from_result(root, result)
            })
            .collect();
        let _ = tx.send(AppMsg::WorkspacesLoaded(reports));
    });
}

/// Load one local patch.
pub(super) fn load_diff(
    workspace: PathBuf,
    path: PathBuf,
    tx: tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    tokio::task::spawn_blocking(move || {
        let result = medulla::workspace::diff(&workspace, &path).map_err(|error| error.to_string());
        let _ = tx.send(AppMsg::WorkspaceDiffLoaded {
            workspace,
            path,
            result,
        });
    });
}

/// Probe open PRs and their check/thread state.
pub(super) fn load_ship(roots: Vec<PathBuf>, tx: tokio::sync::mpsc::UnboundedSender<AppMsg>) {
    tokio::task::spawn_blocking(move || {
        let reports = medulla::ship::ShipClient::new().inspect_workspaces(&roots);
        let _ = tx.send(AppMsg::ShipLoaded(reports));
    });
}

/// Load the selected PR's bounded failed-check excerpt.
pub(super) fn load_ship_log(
    workspace: PathBuf,
    number: u64,
    tx: tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    tokio::task::spawn_blocking(move || {
        let result = medulla::ship::ShipClient::new()
            .failing_log_excerpt(&workspace, number)
            .map_err(|error| error.to_string());
        let _ = tx.send(AppMsg::ShipLogLoaded {
            workspace,
            number,
            result,
        });
    });
}

/// Run an explicit browser action for one PR.
pub(super) fn open_ship_pr(
    workspace: PathBuf,
    number: u64,
    tx: tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    tokio::task::spawn_blocking(move || {
        let status = match medulla::ship::ShipClient::new().open_pr(&workspace, number) {
            Ok(()) => format!("Ship · opened PR #{number}"),
            Err(error) => format!("Ship · {error}"),
        };
        let _ = tx.send(AppMsg::ShipAction(status));
    });
}

/// Create a PR against the repository's canonical upstream remote.
pub(super) fn create_ship_pr(workspace: PathBuf, tx: tokio::sync::mpsc::UnboundedSender<AppMsg>) {
    tokio::task::spawn_blocking(move || {
        let status = match medulla::ship::ShipClient::new().create_pr(&workspace) {
            Ok(url) => format!("Ship · created {url}"),
            Err(error) => format!("Ship · {error}"),
        };
        let _ = tx.send(AppMsg::ShipAction(status));
    });
}
