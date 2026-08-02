//! The Routing Workspaces page's state: reading the list, and adding or
//! removing one of this device's directories.
//!
//! The rows themselves are built in the SDK
//! ([`medulla::ui::workspaces`](crate::ui::workspaces)); everything here is the
//! app-side half — where the primary directory comes from, how an added path is
//! validated, and how a change reaches disk.

use medulla::ui::workspaces::{workspace_rows, WorkspaceRow};

use super::types::App;

impl App {
    /// Every directory the fleet can work in: this device's, then the rest.
    pub(in crate::ui::app) fn workspace_rows(&self) -> Vec<WorkspaceRow> {
        workspace_rows(
            &self.loaded.config.host,
            &self.primary_workspace(),
            &self.fleet_capacity(),
        )
    }

    /// Where delegated work actually runs on this device.
    ///
    /// The running host is the authority: `[host].workspace` is usually blank,
    /// meaning "wherever medulla was launched", and only the host has resolved
    /// that to a real path. Falling back to the config (then the process's own
    /// directory) keeps the page populated when hosting is switched off, where
    /// the setting is still the honest answer to "where would work run".
    fn primary_workspace(&self) -> String {
        if let Some(obs) = &self.host_obs {
            return obs.workspace().to_string();
        }
        let configured = self.loaded.config.host.workspace.trim();
        if !configured.is_empty() {
            return configured.to_string();
        }
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// How many workspaces the page is listing. Test/inspection seam.
    pub fn workspace_row_count(&self) -> usize {
        self.workspace_rows().len()
    }

    /// Whether the selected workspace belongs to another machine. Test seam.
    pub fn selected_workspace_is_remote(&self) -> Option<bool> {
        self.selected_workspace().map(|row| !row.editable)
    }

    /// The workspace under the cursor, if the list is non-empty.
    pub(in crate::ui::app) fn selected_workspace(&self) -> Option<WorkspaceRow> {
        let rows = self.workspace_rows();
        rows.get(self.workspace_index.min(rows.len().saturating_sub(1)))
            .cloned()
    }

    /// Declare `path` as another directory this device can work in.
    ///
    /// The path is canonicalized so the list matches what the host will
    /// advertise; a path that does not resolve is still accepted, because a
    /// directory on a mount that is not up yet is a real thing to declare and
    /// the row flags it as missing rather than refusing it.
    ///
    /// An unresolvable *relative* path is still made absolute against the
    /// current directory before being stored. Persisting `projects/repo`
    /// verbatim would mean the declaration silently names a different place on
    /// the next launch — whatever directory `medulla` happened to start in —
    /// and an advertised relative path is not a location at all to whoever
    /// reads it.
    pub(in crate::ui::app) fn add_workspace(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            self.set_status("Enter a directory path");
            return;
        }
        let resolved = std::fs::canonicalize(path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| absolute(path));
        if self.workspace_rows().iter().any(|r| r.path == resolved) {
            self.set_status(format!("Already declared · {resolved}"));
            return;
        }
        self.loaded.config.host.workspaces.push(resolved.clone());
        self.save_workspaces(format!("Added workspace · {resolved}"));
    }

    /// Stop advertising the selected directory.
    ///
    /// Only this device's extra directories can go: the primary is where work
    /// runs (removing it would leave the host with nowhere to run), and a remote
    /// host's declarations belong to that machine.
    pub(in crate::ui::app) fn remove_selected_workspace(&mut self) {
        let Some(row) = self.selected_workspace() else {
            return;
        };
        if row.primary {
            self.set_status("That is where tasks run — change it in [host].workspace");
            return;
        }
        if !row.editable {
            self.set_status(format!(
                "Declared on {} — remove it there",
                row.host.as_deref().unwrap_or("another machine")
            ));
            return;
        }
        self.loaded.config.host.workspaces.retain(|w| {
            let w = w.trim();
            w != row.path
                && std::fs::canonicalize(w)
                    .map(|p| p.to_string_lossy() != row.path.as_str())
                    .unwrap_or(true)
        });
        self.workspace_index = self.workspace_index.saturating_sub(1);
        self.save_workspaces(format!("Removed workspace · {}", row.path));
    }

    /// Write `[host].workspaces` back, narrating either outcome.
    ///
    /// A silent failure here would be the worst kind: the list on screen would
    /// be right and the file wrong, so the directory would vanish at the next
    /// launch with nothing to explain it.
    fn save_workspaces(&mut self, done: String) {
        let Some(path) = self.config_path.clone() else {
            self.set_status(format!("{done} (this run only — no config file)"));
            return;
        };
        match medulla::config::persist_host_workspaces(&path, &self.loaded.config.host.workspaces) {
            Ok(()) => self.set_status(format!("{done} · restart to advertise it")),
            Err(e) => self.set_status(format!("Could not save the workspace list ({e})")),
        }
    }
}

/// Make `path` absolute against the process's current directory.
///
/// Used only when `canonicalize` failed, which means the directory is not there
/// *yet* — a mount that is not up, a checkout not made. `canonicalize` would
/// also have resolved symlinks and `..`; this cannot, so the stored path is
/// merely absolute rather than fully normalised. That is the honest limit: it
/// fixes "which directory did they mean" without pretending to have inspected
/// something that does not exist.
fn absolute(path: &str) -> String {
    let path = std::path::Path::new(path);
    if path.is_absolute() {
        return path.to_string_lossy().into_owned();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

impl App {
    /// Declare another host on this machine, working in `dir`.
    ///
    /// A blank `dir` accepts the default — the directory this process is
    /// running in — because that is the common case and the one the operator
    /// can see. Anything else is canonicalized so the declaration names a
    /// place rather than whatever `medulla` happened to start in next time.
    ///
    /// Written to config *and* started now, so the host appears in the roster
    /// the way a remote add does rather than waiting for the next launch. The
    /// config write comes first and the start is only requested once it
    /// succeeded — a host running against a declaration that was never saved
    /// disappears on restart with nothing to explain it.
    pub(in crate::ui::app) fn add_local_host(
        &mut self,
        harness: medulla::tinyplace::HarnessProvider,
        dir: &str,
    ) -> Option<super::types::Cmd> {
        let dir = dir.trim();
        let workspace = if dir.is_empty() {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            std::fs::canonicalize(dir)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| absolute(dir))
        };
        if self
            .loaded
            .config
            .hosts
            .iter()
            .any(|h| h.workspace == workspace)
            || self.loaded.config.host.workspace == workspace
        {
            self.set_status(format!("Already hosted · {workspace}"));
            return None;
        }
        let name = std::path::Path::new(&workspace)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let entry = medulla::config::HostSection {
            name,
            // Blank so the address is derived from the name at startup rather
            // than inheriting the primary's and failing to bind.
            address: String::new(),
            workspace: workspace.clone(),
            providers: vec![harness.as_str().to_string()],
            default_provider: harness.as_str().to_string(),
            ..medulla::config::HostSection::default()
        };
        let Some(path) = self.config_path.clone() else {
            self.set_status("No config file to save to");
            return None;
        };
        // Persist a candidate list, and only adopt it once the write succeeded.
        // Pushing first left a host in the in-process config that was never
        // written and never started, and the duplicate check above then refused
        // every retry for that directory — a failure the operator could not
        // clear without restarting.
        let mut hosts = self.loaded.config.hosts.clone();
        hosts.push(entry);
        if let Err(e) = medulla::config::persist_local_hosts(&path, &hosts) {
            self.set_status(format!("Local host was not saved: {e}"));
            return None;
        }
        self.loaded.config.hosts = hosts;
        // Same landing as a remote add, so both kinds end in the same place —
        // and the host is started now rather than at the next launch, so the row
        // appears here the way a remote one does.
        self.focus_routing_subpage("Hosts");
        self.set_status(format!("Starting local host · {workspace}"));
        let index = self.loaded.config.hosts.len().saturating_sub(1);
        self.loaded
            .config
            .hosts
            .last()
            .cloned()
            .map(|host| super::types::Cmd::StartLocalHost {
                host: Box::new(host),
                index,
            })
    }
}
