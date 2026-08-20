//! The Hosts page's state: the `Host → Agents` tree, the cursor over it, and
//! what the operator may do to the row under it.
//!
//! The tree itself is built in the SDK ([`medulla::ui::hosts`]); everything here
//! is the app-side half — where the local hosts come from, how the flat cursor
//! maps onto a two-level tree, and how an edit reaches disk
//! ([`edit`](self::edit)).
//!
//! The page lists hosts, not workers. That distinction is the whole point: one
//! machine now declares one agent per `harness × workspace`, so the flat roster
//! the page used to render was a list of agents with the host level collapsed
//! out of it — and a fleet you cannot see the shape of is one you cannot manage.

use medulla::config::LocalHostRef;
use medulla::ui::hosts::{host_rows, HostAgentRow, HostKind, HostRow};

use super::types::App;

mod edit;

#[cfg(test)]
mod tests;

/// One line on the Hosts page: a host header or one agent under it.
///
/// Both are selectable, because both answer different questions — a host row
/// previews the *machine* (capacity, readiness, budgets), an agent row previews
/// the agent and owns its role toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) struct HostsRow {
    /// Index into the host tree.
    pub host: usize,
    /// Index into that host's agents, or `None` for the host's own header row.
    pub agent: Option<usize>,
}

/// Flatten the tree into the rows the list draws and the cursor walks.
pub(in crate::ui::app) fn flatten(tree: &[HostRow]) -> Vec<HostsRow> {
    let mut rows = Vec::new();
    for (host, entry) in tree.iter().enumerate() {
        rows.push(HostsRow { host, agent: None });
        rows.extend((0..entry.agents.len()).map(|agent| HostsRow {
            host,
            agent: Some(agent),
        }));
    }
    rows
}

impl App {
    /// The hosts this machine runs, as the tab lists them.
    ///
    /// Config is the source, so a host that is declared but not running is still
    /// present — that is the state where the operator most needs to see it. A
    /// *running* primary overrides its own identity from the live observation:
    /// `[host].workspace` is usually blank ("wherever medulla was launched") and
    /// only the running host has resolved it.
    pub(in crate::ui::app) fn local_host_refs(&self) -> Vec<LocalHostRef> {
        let mut hosts =
            medulla::config::local_hosts(&self.loaded.config.host, &self.loaded.config.hosts);
        if let (Some(observation), Some(primary)) = (self.host_obs.as_ref(), hosts.first_mut()) {
            primary.id = observation.address().to_string();
            primary.workspace = observation.workspace().to_string();
            primary.name = medulla::config::local_host_name(
                &self.loaded.config.host,
                observation.workspace(),
                true,
            );
        }
        // A host this device is not serving is not a host. It stays listed only
        // while it still holds something — declared agents, or roster entries
        // that outlived the switch — because dropping those would hide agents
        // the operator wrote down rather than tidying the page.
        let sections = std::iter::once(&self.loaded.config.host).chain(&self.loaded.config.hosts);
        let hosting = self.host_obs.is_some();
        hosts
            .into_iter()
            .zip(sections)
            .filter(|(host, section)| {
                section.enabled
                    || (host.primary && hosting)
                    || self.declares_agent_on(&host.id)
                    || self
                        .runtime
                        .workers()
                        .iter()
                        .any(|worker| worker.address.trim() == host.id)
            })
            .map(|(host, _)| host)
            .collect()
    }

    /// Whether this machine's config declares any agent on `host_id`.
    fn declares_agent_on(&self, host_id: &str) -> bool {
        self.loaded
            .config
            .fleet
            .agent_declarations
            .iter()
            .any(|declaration| declaration.on_host(host_id))
    }

    /// The `Host → Agents` tree the page renders.
    pub(in crate::ui::app) fn host_tree(&self) -> Vec<HostRow> {
        host_rows(
            &self.runtime.workers(),
            &self.loaded.config.fleet.agent_declarations,
            &self.local_host_refs(),
        )
    }

    /// The flattened rows, with the cursor clamped into range.
    ///
    /// Returned together because every caller needs both and clamping against a
    /// stale length is how a cursor ends up pointing at a row that is no longer
    /// there.
    pub(in crate::ui::app) fn hosts_view(&self) -> (Vec<HostRow>, Vec<HostsRow>, usize) {
        let tree = self.host_tree();
        let rows = flatten(&tree);
        let selected = self.host_index.min(rows.len().saturating_sub(1));
        (tree, rows, selected)
    }

    /// How many rows the page lists. Test/inspection seam.
    pub fn hosts_row_count(&self) -> usize {
        flatten(&self.host_tree()).len()
    }

    /// The host under the cursor — the header itself, or the host of the agent
    /// row the cursor is on.
    pub(in crate::ui::app) fn selected_host_row(&self) -> Option<HostRow> {
        let (tree, rows, selected) = self.hosts_view();
        tree.get(rows.get(selected)?.host).cloned()
    }

    /// The agent under the cursor, when the cursor is on an agent row.
    /// Also a test/inspection seam.
    pub fn selected_host_agent(&self) -> Option<HostAgentRow> {
        let (tree, rows, selected) = self.hosts_view();
        let row = rows.get(selected)?;
        tree.get(row.host)?.agents.get(row.agent?).cloned()
    }

    /// Whether the create-agent flow is on screen.
    /// Test/inspection seam.
    pub fn session_picker_open(&self) -> bool {
        self.session_picker.is_some()
    }

    /// Whether the preview's role toggles hold the arrows.
    ///
    /// The Hosts page has two cursors on one screen — the tree and the toggle
    /// list — and which of them a keypress reaches is not legible from the
    /// rendered buffer alone: an agent preview is drawn either way.
    /// Test/inspection seam.
    pub fn host_roles_focused(&self) -> bool {
        self.host_roles_focus
    }

    /// Whether the cursor is on a host header rather than an agent.
    /// Test/inspection seam.
    pub fn hosts_cursor_on_host(&self) -> bool {
        let (_, rows, selected) = self.hosts_view();
        rows.get(selected).is_some_and(|row| row.agent.is_none())
    }

    /// The roster entry the cursor's row acts on: the agent itself, or the entry
    /// that probed the machine when the cursor is on a host header.
    ///
    /// This is what `Enter`, `d`, `e` and `r` target — every one of them is a
    /// mutation of a *roster* entry, and a host that has none (declared here,
    /// nothing running) has nothing for them to act on.
    pub(in crate::ui::app) fn selected_host(&self) -> Option<medulla::runtime::WorkerInfo> {
        let (tree, rows, selected) = self.hosts_view();
        let row = rows.get(selected)?;
        let host = tree.get(row.host)?;
        let id = match row.agent {
            Some(agent) => {
                let agent = host.agents.get(agent)?;
                agent.live.then(|| agent.agent_id.clone())?
            }
            None => host.detail_worker.clone()?,
        };
        self.runtime
            .workers()
            .into_iter()
            .find(|worker| worker.id == id)
    }

    /// Whether the row under the cursor may be edited from here.
    ///
    /// The v1 capability split: a remote host's agents are declared on that
    /// machine, so this end is a viewer. Orchestrator dispatch is untouched by
    /// it — only what the *operator* can change from this terminal.
    /// Test/inspection seam.
    pub fn selected_host_is_local(&self) -> bool {
        self.selected_host_row()
            .is_some_and(|host| host.kind == HostKind::Local)
    }
}
