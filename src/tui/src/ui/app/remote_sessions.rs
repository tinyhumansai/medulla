//! Remote sessions as the rail and the pane already understand them.
//!
//! # Why a synthesized `SessionRow` rather than a second row type
//!
//! The rail, the pane, the close confirmation, the attention badge and the
//! keyboard all speak [`SessionRow`]. Adding a parallel remote type would mean
//! teaching every one of them a second vocabulary, and the two would drift —
//! which is the whole failure this avoids. So a remote session is translated
//! *into* a `SessionRow` once, here, and everything downstream is unchanged.
//!
//! The fields a remote session genuinely does not have are left honestly empty
//! rather than guessed at. Checkout tracking, the launch commit and the MCP grant
//! key all describe things on *this* filesystem; inventing values for them would
//! make the Changes view claim to know about a repository it cannot see.
//!
//! # Ids
//!
//! A local id stays bare (`w_7`), so every existing path, status message and test
//! is byte-for-byte unchanged. A remote one is `<hostId>/w_7`. The separator is
//! the whole resolution rule — see [`SessionRef`].

use medulla::protocol::{RemoteSessionRow, RemoteSessionState};

use super::types::App;
use crate::worker::pty::{PtyState, SessionControl, SessionOrigin, SessionRow};

/// The character separating a host id from a session id.
///
/// A path separator on purpose: it cannot occur in a `w_…` id, and it reads as
/// "there, then this" — which is exactly what a remote session id means.
pub const HOST_SEPARATOR: char = '/';

/// A session id, and where it lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRef<'a> {
    /// On this machine. The id is exactly what the pty manager knows it by.
    Local(&'a str),
    /// On another machine.
    Remote {
        /// The `[[remoteHosts]]` id.
        host: &'a str,
        /// The session id as *that host* knows it — never the prefixed form.
        session: &'a str,
    },
}

impl<'a> SessionRef<'a> {
    /// Resolve an id the UI is holding.
    ///
    /// Anything without a separator is local, which is what keeps every existing
    /// caller working untouched.
    pub fn parse(id: &'a str) -> Self {
        match id.split_once(HOST_SEPARATOR) {
            Some((host, session)) if !host.is_empty() && !session.is_empty() => {
                SessionRef::Remote { host, session }
            }
            _ => SessionRef::Local(id),
        }
    }
}

/// The prefixed id a remote session is filed under on this side.
pub fn qualify(host_id: &str, session_id: &str) -> String {
    format!("{host_id}{HOST_SEPARATOR}{session_id}")
}

/// Translate one remote session into the row every surface already reads.
/// `since` is when this client first saw the row's current cue — never the
/// sender's own timestamp, which is a reading from another machine's clock.
pub fn session_row(host_id: &str, remote: &RemoteSessionRow, since: i64) -> SessionRow {
    SessionRow {
        id: qualify(host_id, &remote.id),
        label: remote.label.clone(),
        provider: medulla::protocol::HarnessProvider::from_wire(&remote.provider)
            .unwrap_or(medulla::protocol::HarnessProvider::Shell),
        preset: remote.preset.clone(),
        state: match remote.state {
            RemoteSessionState::Running => PtyState::Running,
            RemoteSessionState::Exited { code } => PtyState::Exited { code },
            RemoteSessionState::Failed => PtyState::Failed,
        },
        cwd: remote.cwd.clone(),
        // Everything below describes this machine's filesystem, and a remote
        // session is not on it. Left empty rather than invented: the Changes
        // view reads these, and a fabricated checkout would have it claim to
        // know a repository it cannot see.
        checkout: medulla::ui::checkout::Checkout::default(),
        launch_root: None,
        launch_commit: None,
        launch_checkout_identity: None,
        session_id: None,
        thread_name: remote.thread_name.clone(),
        started_at: remote.started_at,
        last_output_at: remote.last_output_at,
        last_error: remote.last_error.clone(),
        busy: remote.busy,
        // Always the operator's. A remote session exists because somebody asked
        // for it in the picker, and nothing dispatches into one — so the
        // orchestrator must never see it as available.
        control: SessionControl::User,
        origin: SessionOrigin::User,
        retained: false,
        closed_by_request: false,
        name: remote.name.clone(),
        attention: remote.attention.as_ref().map(|attention| {
            crate::worker::pty::HarnessAttention {
                // An unrecognized kind becomes `Choice` — the "it is asking, and
                // we cannot name what for" cue. Dropping the whole cue instead
                // would hide a harness that is genuinely stuck, which is the one
                // thing the rail exists to surface.
                kind: crate::worker::pty::AttentionKind::from_wire(&attention.kind)
                    .unwrap_or(crate::worker::pty::AttentionKind::Choice),
                what: attention.summary.clone(),
                // Stamped by this client when the cue was first seen, not taken
                // from the sender: the rail subtracts this from *our* clock to
                // say how long a harness has been waiting, so a skewed host
                // would otherwise report a fresh prompt as stuck for hours.
                since,
            }
        }),
        working: remote.working,
        mcp_grant_session: None,
    }
}

impl App {
    /// Every remote session, as rows the rail can place.
    pub(in crate::ui::app) fn remote_session_rows(&self) -> Vec<SessionRow> {
        self.remote_hosts
            .iter()
            .flat_map(|(host_id, state)| {
                state.rows.iter().map(move |remote| {
                    let since = state
                        .attention_since
                        .get(&remote.id)
                        .map(|(_, at)| *at)
                        .unwrap_or(remote.last_output_at);
                    session_row(host_id, remote, since)
                })
            })
            .collect()
    }
}

impl App {
    /// The screen for `id`, wherever the session runs.
    ///
    /// The one lookup that makes a remote pane possible: a local id reaches the
    /// emulator directly, a remote one reads the grid the client has been
    /// folding frames into. Both answer a [`ScreenSnapshot`], so every renderer
    /// downstream — `screen_lines`, the cursor, the hit map — is shared verbatim
    /// and there is no second drawing path to keep in step.
    pub(in crate::ui::app) fn session_screen(
        &self,
        id: &str,
    ) -> Option<crate::worker::pty::ScreenSnapshot> {
        match SessionRef::parse(id) {
            SessionRef::Local(id) => self.local_sessions.as_ref()?.screen(id),
            SessionRef::Remote { host, session } => {
                let state = self.remote_hosts.get(host)?;
                match state.screen.as_ref() {
                    Some((watching, snapshot)) if watching == session => Some(snapshot.clone()),
                    // Watching something else, or nothing yet. The pane draws
                    // its frame and waits rather than showing another session's
                    // screen under this one's title.
                    _ => None,
                }
            }
        }
    }

    /// Match the pane's geometry onto the session, wherever it runs.
    ///
    /// Called every frame. The local path short-circuits when the size already
    /// matches; the remote path is deduplicated here for the same reason, since
    /// a resize per frame would be a datagram per frame on an idle session.
    pub(in crate::ui::app) fn session_fit(&mut self, id: &str, cols: u16, rows: u16) {
        match SessionRef::parse(id) {
            SessionRef::Local(id) => {
                if let Some(local) = self.local_sessions.as_ref() {
                    local.fit(id, cols, rows);
                }
            }
            SessionRef::Remote { host, .. } => {
                let Some(state) = self.remote_hosts.get_mut(host) else {
                    return;
                };
                if state.pane_size == Some((cols, rows)) {
                    return;
                }
                state.pane_size = Some((cols, rows));
                // Dropped deliberately: this runs on the render pass, where a
                // disconnected host is already visible on its rail row and there
                // is nothing a failed resize would add. The size is recorded
                // either way, so a reconnect sends the current geometry.
                let _ = state.request(crate::remote::client::RemoteRequest::Resize { cols, rows });
            }
        }
    }

    /// Send already-encoded bytes to a session, wherever it runs.
    ///
    /// # Errors
    ///
    /// When the session is unknown, has exited, or — for a remote one — its host
    /// is no longer connected. Surfaced rather than swallowed: an attached pane
    /// whose keystrokes vanish is worse than one that says why.
    pub(in crate::ui::app) fn session_write(
        &mut self,
        id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        match SessionRef::parse(id) {
            SessionRef::Local(id) => self
                .local_sessions
                .as_ref()
                .ok_or_else(|| "this device is not hosting".to_string())?
                .write(id, bytes),
            SessionRef::Remote { host, .. } => {
                let state = self
                    .remote_hosts
                    .get_mut(host)
                    .ok_or_else(|| format!("{host} is not connected"))?;
                state.request(crate::remote::client::RemoteRequest::Input(bytes.to_vec()))
            }
        }
    }

    /// Stream a remote session's screen, so the pane has something to draw.
    ///
    /// A no-op for a local session, which needs no subscription — its emulator
    /// is right here. Called when the pane's selection changes, and cheap to
    /// repeat: the host ignores a subscribe for what it is already sending.
    pub(in crate::ui::app) fn session_watch(&mut self, id: &str) {
        let SessionRef::Remote { host, session } = SessionRef::parse(id) else {
            return;
        };
        let session = session.to_string();
        let Some(state) = self.remote_hosts.get_mut(host) else {
            return;
        };
        if state.watching.as_deref() == Some(session.as_str()) {
            return;
        }
        state.watching = Some(session.clone());
        // Cleared rather than kept: the held grid belongs to the session we just
        // stopped watching, and drawing it under the new session's title would be
        // a lie the operator has no way to notice.
        state.screen = None;
        // Also cleared: `pane_size` dedups `session_fit`'s resize against the
        // *host*, not the session it was last sent for, so without this a
        // switch to a session that happens to need the same geometry the
        // previous one already had would never be resized — it stays at
        // whatever size it was opened with even though the pane wants
        // something else. Clearing it makes the very next `session_fit` call
        // resend the resize unconditionally for the newly watched session.
        state.pane_size = None;
        // Also dropped on the render pass, and for the same reason. The
        // subscription is re-sent on the next frame that draws this pane, so a
        // host that comes back starts streaming without another gesture.
        let _ = state.request(crate::remote::client::RemoteRequest::Watch(session));
    }
}

impl App {
    /// The row for `id`, wherever the session runs.
    ///
    /// The pane's title, the close confirmation and the attention cue all read
    /// this, so routing it here is what keeps them working on a remote session
    /// without any of them learning a second vocabulary.
    pub(in crate::ui::app) fn session_row(&self, id: &str) -> Option<SessionRow> {
        match SessionRef::parse(id) {
            SessionRef::Local(id) => self.local_sessions.as_ref()?.sessions.row(id),
            SessionRef::Remote { host, session } => {
                let state = self.remote_hosts.get(host)?;
                state.rows.iter().find(|row| row.id == session).map(|row| {
                    let since = state
                        .attention_since
                        .get(&row.id)
                        .map(|(_, at)| *at)
                        .unwrap_or(row.last_output_at);
                    session_row(host, row, since)
                })
            }
        }
    }
}

impl App {
    /// Whether `id`'s emulator has bracketed paste on.
    ///
    /// The mode belongs to the emulator the child is talking to, so for a remote
    /// session it is the far side's answer — carried on its session row
    /// precisely because *this* client is the one encoding the paste, and a
    /// paste bracketed wrongly is something harnesses notice.
    pub(in crate::ui::app) fn session_bracketed_paste(&self, id: &str) -> bool {
        match SessionRef::parse(id) {
            SessionRef::Local(id) => self
                .local_sessions
                .as_ref()
                .and_then(|local| local.sessions.bracketed_paste(id))
                .unwrap_or(false),
            SessionRef::Remote { host, session } => self
                .remote_hosts
                .get(host)
                .and_then(|state| state.rows.iter().find(|row| row.id == session))
                .is_some_and(|row| row.bracketed_paste),
        }
    }
}

impl App {
    /// Fold a freshly received screen into the host that sent it.
    ///
    /// Ignored when it names a session the client is no longer watching: the
    /// host may already have had a frame in flight when the pane moved, and
    /// drawing it under the new session's title would be a lie the operator has
    /// no way to notice.
    pub fn remote_host_screen(
        &mut self,
        host_id: &str,
        session_id: &str,
        snapshot: crate::worker::pty::ScreenSnapshot,
    ) {
        let Some(state) = self.remote_hosts.get_mut(host_id) else {
            return;
        };
        if state.watching.as_deref() != Some(session_id) {
            return;
        }
        state.screen = Some((session_id.to_string(), snapshot));
    }

    /// Move the cursor onto a session that just started on a remote host.
    pub fn select_remote_session(&mut self, host_id: &str, session_id: &str) {
        let id = qualify(host_id, session_id);
        self.tab_index = super::types::tab_pos("Sessions");
        self.select_session_row(&id);
    }
}

#[cfg(test)]
impl App {
    /// Whether this device is hosting, for tests that assert the remote path
    /// works without it.
    pub(in crate::ui::app) fn local_sessions_for_test(
        &self,
    ) -> Option<&crate::ui::harness_pane::LocalSessions> {
        self.local_sessions.as_ref()
    }

    /// [`own_session_rows`](crate::ui::app::App::own_session_rows) for tests.
    pub(in crate::ui::app) fn own_session_rows_for_test(&self) -> Vec<SessionRow> {
        self.own_session_rows()
    }

    /// [`session_watch`](Self::session_watch) for tests.
    pub(in crate::ui::app) fn session_watch_for_test(&mut self, id: &str) {
        self.session_watch(id);
    }

    /// [`session_screen`](Self::session_screen) for tests.
    pub(in crate::ui::app) fn session_screen_for_test(
        &self,
        id: &str,
    ) -> Option<crate::worker::pty::ScreenSnapshot> {
        self.session_screen(id)
    }

    /// [`remote_choices`](crate::ui::app::App::remote_choices) for tests.
    pub(in crate::ui::app) fn remote_choices_for_test(
        &self,
        host_id: &str,
    ) -> Vec<crate::ui::harness_pane::HarnessChoice> {
        self.remote_choices(host_id)
    }

    /// [`attach_to_session`](crate::ui::app::App::attach_to_session) for tests.
    pub(in crate::ui::app) fn attach_to_session_for_test(&mut self, id: &str) {
        self.attach_to_session(id);
    }

    /// Drop this device's hosting surface, standing in for `MEDULLA_HOST=0`.
    pub(in crate::ui::app) fn clear_local_sessions_for_test(&mut self) {
        self.local_sessions = None;
    }

    /// [`session_row`](crate::ui::app::App::session_row) for tests.
    pub(in crate::ui::app) fn session_row_for_test(&self, id: &str) -> Option<SessionRow> {
        self.session_row(id)
    }

    /// [`session_write`](Self::session_write) for tests.
    pub(in crate::ui::app) fn session_write_for_test(
        &mut self,
        id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.session_write(id, bytes)
    }
}
