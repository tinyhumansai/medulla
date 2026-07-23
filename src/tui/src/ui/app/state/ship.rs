//! Ship-section state folding, selection, and explicit action commands.

use crate::ui::app::{App, Cmd};

impl App {
    /// Replace Ship rows and clamp the independent PR selection.
    pub fn set_ship_reports(&mut self, reports: Vec<medulla::ship::WorkspaceShipReport>) {
        self.repo.ship_reports = reports;
        self.repo.ship_loading = false;
        self.repo.ship_index = self
            .repo
            .ship_index
            .min(self.ship_prs().len().saturating_sub(1));
        if self.ship_prs().is_empty() {
            self.repo.ship_log_key = None;
            self.repo.ship_log.clear();
        }
    }

    /// Keep the last good Ship view visible while a read probe runs.
    pub fn set_ship_loading(&mut self) {
        self.repo.ship_loading = true;
    }

    /// Flatten open PRs into their selectable workspace/summary pairs.
    pub(super) fn ship_prs(&self) -> Vec<(std::path::PathBuf, medulla::ship::PrSummary)> {
        self.repo
            .ship_reports
            .iter()
            .flat_map(|report| match &report.state {
                medulla::ship::ShipState::Ready(rows) => rows
                    .iter()
                    .cloned()
                    .map(|row| (report.root.clone(), row))
                    .collect(),
                medulla::ship::ShipState::GhUnavailable(_) => Vec::new(),
            })
            .collect()
    }

    /// Request the selected PR's failed-check excerpt.
    pub fn selected_ship_log_cmd(&self) -> Option<Cmd> {
        let (workspace, row) = self.ship_prs().get(self.repo.ship_index)?.clone();
        Some(Cmd::LoadShipLog {
            workspace,
            number: row.number,
        })
    }

    /// Move the PR cursor without disturbing dirty-file selection.
    pub(in crate::ui::app) fn move_ship_pr(&mut self, up: bool) -> Option<Cmd> {
        let max = self.ship_prs().len().saturating_sub(1);
        self.repo.ship_index = if up {
            self.repo.ship_index.saturating_sub(1)
        } else {
            (self.repo.ship_index + 1).min(max)
        };
        self.selected_ship_log_cmd()
    }

    /// Store a selected PR's failed-check excerpt or typed failure.
    pub fn set_ship_log(
        &mut self,
        workspace: std::path::PathBuf,
        number: u64,
        result: Result<String, String>,
    ) {
        self.repo.ship_log_key = Some((workspace, number));
        self.repo.ship_log = result.unwrap_or_else(|error| error);
    }

    /// Explicit browser action for the selected PR.
    pub(in crate::ui::app) fn selected_ship_open_cmd(&self) -> Option<Cmd> {
        let (workspace, row) = self.ship_prs().get(self.repo.ship_index)?.clone();
        Some(Cmd::OpenShipPr {
            workspace,
            number: row.number,
        })
    }

    /// Prefer the selected PR's workspace, then the first configured repo.
    pub(in crate::ui::app) fn ship_create_cmd(&self) -> Option<Cmd> {
        let workspace = self
            .ship_prs()
            .get(self.repo.ship_index)
            .map(|(root, _)| root.clone())
            .or_else(|| self.repo.reports.first().map(|report| report.root.clone()))
            .or_else(|| self.loaded.workflow_workspaces().into_iter().next())?;
        Some(Cmd::CreateShipPr(workspace))
    }
}
