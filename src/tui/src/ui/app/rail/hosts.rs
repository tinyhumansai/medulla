//! The `Host → Agent` projection the Sessions rail places its lanes onto.
//!
//! This used to back the Hosts tab, which drew the tree directly and let an
//! operator edit it. That tab is gone — its pages managed an orchestration
//! engine that no longer exists — but the projection is not a management
//! surface: it is what gives a session its lane, hence its transcript and its
//! place in the rail's order. So the three reads the rail needs live here, and
//! nothing writes.

use medulla::config::LocalHostRef;
use medulla::ui::hosts::{host_rows, HostRow};

use super::super::types::App;

impl App {
    /// The hosts this machine runs, as the rail places lanes on them.
    ///
    /// Config is the source, so a host that is declared but not running is still
    /// present. A *running* primary overrides its own identity from the live
    /// observation: `[host].workspace` is usually blank ("wherever medulla was
    /// launched") and only the running host has resolved it.
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
        // the operator wrote down rather than tidying the rail.
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

    /// The `Host → Agents` tree the rail places its folded lanes onto.
    ///
    /// Local hosts first, then the configured remote machines. The order is the
    /// same one the picker offers: this device is where a session goes when
    /// nobody says otherwise, so it leads.
    pub(in crate::ui::app) fn host_tree(&self) -> Vec<HostRow> {
        let mut rows = host_rows(
            &self.runtime.workers(),
            &self.loaded.config.fleet.agent_declarations,
            &self.local_host_refs(),
        );
        rows.extend(self.remote_host_rows());
        rows
    }
}

impl App {
    /// The remote machines in the `Host → Agent` projection.
    ///
    /// Config is the source, exactly as it is for local hosts, so a host appears
    /// here whether or not it has been connected to.
    ///
    /// Note that being *in the projection* is not the same as being drawn: the
    /// rail drops any section with no sessions in it
    /// ([`rail_rows_in`](super::App::rail_rows_in)), which is the same
    /// progressive-disclosure rule that keeps an idle local host from claiming a
    /// header. So a configured-but-idle remote machine is reached through the
    /// session picker, and earns a lane once it has something running on it.
    pub(in crate::ui::app) fn remote_host_rows(&self) -> Vec<HostRow> {
        medulla::config::remote_hosts(&self.loaded.config.remote_hosts)
            .into_iter()
            .map(|host| HostRow {
                // Sanitized: this is the label the rail renders, and
                // `host.name`/`host.id` are untrusted configuration — see
                // `sanitize_for_rail`. `id` itself stays raw: it is a routing
                // key compared against worker addresses and agent
                // declarations elsewhere, never rendered on its own.
                label: sanitize_for_rail(match host.name.trim() {
                    "" => &host.id,
                    name => name,
                }),
                id: host.id,
                kind: medulla::ui::hosts::HostKind::Remote,
                // Empty rather than guessed: a remote host's agents are declared
                // in its own config, on its own machine. Its live sessions ride
                // the session list instead.
                agents: Vec::new(),
                detail_worker: None,
            })
            .collect()
    }
}

/// Strip control characters from configuration before it reaches a rail span.
///
/// `[[remoteHosts]]` name and id come from `medulla.toml`, which the operator
/// wrote but which the render path must treat as untrusted the same way it
/// treats a harness's own stdout: ratatui does not neutralize an ESC or OSC
/// sequence in a rendered string, so a crafted host name reaches the terminal
/// and is interpreted there, not on screen. See
/// `render::workflows::node_preview::live::inline_text` for the same trim.
fn sanitize_for_rail(value: &str) -> String {
    value.trim().chars().filter(|ch| !ch.is_control()).collect()
}
