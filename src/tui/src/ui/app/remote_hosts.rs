//! What the client knows about each `[[remoteHosts]]` machine.
//!
//! Deliberately a *cache*, not a connection. The connection itself lives on the
//! event loop's side, because bootstrapping one means running `ssh` and waiting —
//! and the render pass must never wait for anything. What the App holds is the
//! last thing each host said about itself, which is enough to draw a rail lane
//! and fill a picker.
//!
//! # One connection per host, opened when it is needed
//!
//! Not at startup: a configured host that is switched off, or on a network the
//! laptop is not on, must cost nothing until somebody actually asks for it. So a
//! host sits in [`RemoteHostStatus::Idle`] until the operator picks it, and only
//! then is `ssh` run. After that the one link carries every session on that
//! machine — the expensive part is the bootstrap, and it happens once per host
//! rather than once per shell.

use medulla::protocol::{RemoteCapabilities, RemoteSessionRow};

/// Where a remote host is in its connection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::ui::app) enum RemoteHostStatus {
    /// Configured and never dialled. The ordinary state, and free.
    #[default]
    Idle,
    /// `ssh` is running, or the link has not answered yet.
    Connecting,
    /// Answered, and its capabilities are known.
    Live,
    /// The bootstrap failed. Terminal until the operator asks again, and it
    /// carries the reason because "could not connect" on its own is never enough
    /// to act on — the useful cases are an unknown host key, a missing binary
    /// and a refused login, and each has a different fix.
    Failed(String),
}

/// One remote machine as the UI sees it between connections.
#[derive(Debug, Clone, Default)]
pub(in crate::ui::app) struct RemoteHostState {
    /// Where its connection is.
    pub(in crate::ui::app) status: RemoteHostStatus,
    /// What it said it can do. Empty until it answers.
    pub(in crate::ui::app) capabilities: RemoteCapabilities,
    /// Its sessions, as last reported.
    pub(in crate::ui::app) rows: Vec<RemoteSessionRow>,
    /// The session whose screen is streaming, and the screen itself.
    ///
    /// One at a time, and that is the transport speaking rather than a
    /// simplification: channel 1 carries a single synchronised grid per peer, so
    /// two streams to one host would overwrite each other every frame. Every
    /// other session stays live and keeps appearing in [`rows`](Self::rows),
    /// which rides the reliable channel.
    pub(in crate::ui::app) watching: Option<String>,
    /// The watched session's screen, folded from the frames received so far.
    pub(in crate::ui::app) screen: Option<(String, crate::worker::pty::ScreenSnapshot)>,
    /// When each session's current attention cue was first *seen here*, by
    /// session id, alongside the cue it belongs to.
    ///
    /// The sender's own `since` cannot cross: it is a reading from that
    /// machine's clock, and the rail subtracts this from *ours* to say how long
    /// a harness has been waiting. A skewed host would otherwise show a fresh
    /// prompt as stuck for hours, or pinned at zero forever.
    ///
    /// The cue is stored with it so the stamp survives while the same cue holds
    /// and resets when a different one replaces it — which is what makes "how
    /// long has it been asking this" true rather than "how long since the last
    /// frame".
    pub(in crate::ui::app) attention_since: std::collections::HashMap<String, (String, i64)>,
    /// The size the pane last told this host about, so an unchanged geometry
    /// does not cost a datagram every frame.
    pub(in crate::ui::app) pane_size: Option<(u16, u16)>,
    /// How to reach the task that owns this host's connection.
    ///
    /// `None` before the first dial. The task outlives every session on the
    /// machine, which is what makes one bootstrap serve all of them.
    pub(in crate::ui::app) requests:
        Option<tokio::sync::mpsc::UnboundedSender<crate::remote::client::RemoteRequest>>,
}

impl RemoteHostState {
    /// Ask this host's connection to do something.
    ///
    /// # Errors
    ///
    /// When the host has never been dialled, or its task has stopped — both of
    /// which mean the request cannot be carried out and the caller should say so
    /// rather than appear to have done it.
    pub(in crate::ui::app) fn request(
        &mut self,
        request: crate::remote::client::RemoteRequest,
    ) -> Result<(), String> {
        let sender = self
            .requests
            .as_ref()
            .ok_or_else(|| "not connected".to_string())?;
        sender
            .send(request)
            .map_err(|_| "the connection has stopped".to_string())
    }
}

impl RemoteHostState {
    /// Whether this host is worth asking to connect.
    ///
    /// A failed host is, on purpose: the operator picking it again is exactly
    /// how they say "try that once more", and requiring a separate gesture to
    /// clear the failure would be a step with no meaning of its own.
    pub(in crate::ui::app) fn should_dial(&self) -> bool {
        matches!(
            self.status,
            RemoteHostStatus::Idle | RemoteHostStatus::Failed(_)
        )
    }

    /// A short phrase for the rail and the picker.
    pub(in crate::ui::app) fn summary(&self) -> String {
        match &self.status {
            RemoteHostStatus::Idle => "not connected".to_string(),
            RemoteHostStatus::Connecting => "connecting…".to_string(),
            RemoteHostStatus::Live => match self.rows.len() {
                0 => "connected".to_string(),
                1 => "1 session".to_string(),
                n => format!("{n} sessions"),
            },
            RemoteHostStatus::Failed(reason) => reason.clone(),
        }
    }
}

use super::types::App;
use crate::ui::harness_pane::HarnessChoice;

impl App {
    /// What `host_id` said it can start, translated into picker rows.
    ///
    /// Empty when the host has not answered yet, which the picker reads as "ask
    /// it" rather than "it offers nothing" — the two are different, and only the
    /// second is worth telling the operator about.
    ///
    /// A remote host's list comes from the host and never from this machine's
    /// own: offering a `claude` row for a box that has none would break the
    /// picker's standing promise that every row it shows can actually start.
    pub(in crate::ui::app) fn remote_choices(&self, host_id: &str) -> Vec<HarnessChoice> {
        let Some(state) = self.remote_hosts.get(host_id) else {
            return Vec::new();
        };
        state
            .capabilities
            .harnesses
            .iter()
            .map(|choice| {
                HarnessChoice::remote(
                    // Unrecognized providers are still offered, as a shell's
                    // glyph: the host says it can start the thing, and this
                    // build not knowing the name is a reason to draw it plainly,
                    // not a reason to hide a row that would work.
                    medulla::protocol::HarnessProvider::from_wire(&choice.provider)
                        .unwrap_or(medulla::protocol::HarnessProvider::Shell),
                    crate::ui::harness_pane::RemoteChoice {
                        id: choice.id.clone(),
                        display_name: choice.display_name.clone(),
                    },
                )
            })
            .collect()
    }

    /// The cached state for `host_id`, if it has any.
    pub(in crate::ui::app) fn remote_host(&self, host_id: &str) -> Option<&RemoteHostState> {
        self.remote_hosts.get(host_id)
    }

    /// Record that a host is being dialled.
    pub(in crate::ui::app) fn remote_host_connecting(&mut self, host_id: &str) {
        self.remote_hosts
            .entry(host_id.to_string())
            .or_default()
            .status = RemoteHostStatus::Connecting;
    }

    /// Fold in what a host answered with.
    pub fn remote_host_connected(&mut self, host_id: &str, capabilities: RemoteCapabilities) {
        let state = self.remote_hosts.entry(host_id.to_string()).or_default();
        state.status = RemoteHostStatus::Live;
        state.capabilities = capabilities;
    }

    /// Record that a host could not be reached, and why.
    pub fn remote_host_failed(&mut self, host_id: &str, reason: String) {
        self.remote_hosts
            .entry(host_id.to_string())
            .or_default()
            .status = RemoteHostStatus::Failed(reason);
    }

    /// Replace a host's session list.
    pub fn remote_host_sessions(&mut self, host_id: &str, rows: Vec<RemoteSessionRow>) {
        let now = medulla::clock::now_millis();
        let state = self.remote_hosts.entry(host_id.to_string()).or_default();
        // Stamped on arrival, and kept while the same cue holds. A row whose cue
        // is unchanged keeps the moment we first saw it; a new or cleared cue
        // starts again.
        state.attention_since.retain(|id, _| {
            rows.iter()
                .any(|row| &row.id == id && row.attention.is_some())
        });
        for row in &rows {
            let Some(attention) = row.attention.as_ref() else {
                continue;
            };
            let cue = format!("{}:{}", attention.kind, attention.summary);
            match state.attention_since.get(&row.id) {
                Some((seen, _)) if seen == &cue => {}
                _ => {
                    state.attention_since.insert(row.id.clone(), (cue, now));
                }
            }
        }
        state.rows = rows;
    }
}

impl App {
    /// Leave `cmd` for [`on_event`](App::on_event) to collect.
    ///
    /// For handlers with no way to return one — see
    /// [`pending_cmds`](crate::ui::app::types::App::pending_cmds).
    pub(in crate::ui::app) fn queue_cmd(&mut self, cmd: crate::ui::app::types::Cmd) {
        self.pending_cmds.push_back(cmd);
    }

    /// Take the next queued command, if any.
    pub(in crate::ui::app) fn take_queued_cmd(&mut self) -> Option<crate::ui::app::types::Cmd> {
        self.pending_cmds.pop_front()
    }
}

impl App {
    /// Record how to reach a host's freshly started connection task.
    pub fn remote_host_dialling(
        &mut self,
        host_id: &str,
        requests: tokio::sync::mpsc::UnboundedSender<crate::remote::client::RemoteRequest>,
    ) {
        let state = self.remote_hosts.entry(host_id.to_string()).or_default();
        state.status = RemoteHostStatus::Connecting;
        state.requests = Some(requests);
        // `watching` names a session on the connection that just failed — the
        // replacement task above knows nothing about it, and
        // `session_watch` only re-sends `Watch` when the id *changes*. Left
        // alone, a pane that was already selected on this host would stay
        // selected but never actually resubscribe: the operator sees the same
        // pane, permanently stale, because nothing ever asks the new
        // connection to stream it again. Clearing it (rather than the
        // selection App holds elsewhere) makes the very next render's
        // `session_watch` call for the still-selected session look like a
        // fresh watch and resend it — over the new connection this time. The
        // held screen and cached pane size are stale for the same reason and
        // cleared alongside it.
        state.watching = None;
        state.screen = None;
        state.pane_size = None;
    }
}
