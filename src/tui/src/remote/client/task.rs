//! The long-lived task that owns one host's connection.
//!
//! The App is synchronous and its render pass must never wait, but a connection
//! is asynchronous and occasionally slow — `ssh` at the start, then a link that
//! may be waiting on a peer that has gone quiet. So the connection lives out here
//! and the two talk through channels: requests down, updates up.
//!
//! One task per host, and it outlives every session on that machine. That is
//! what "one connection, opened when needed" means in practice: the expensive
//! part is the bootstrap, so it happens once and everything else is multiplexed
//! over the link it produced.

use std::time::Duration;

use medulla::config::RemoteHostSection;
use medulla::protocol::{RemoteCapabilities, RemoteSessionRow};
use medulla_link::keys::NodeId;

use super::RemoteHost;
use crate::worker::pty::ScreenSnapshot;

/// How often the task pumps its link when nothing has been asked of it.
///
/// The screen channel is latest-wins and the sampler on the far side is already
/// rate-limited, so this is a floor on latency rather than a sampling rate: a
/// frame that has arrived is folded on the next tick.
const TICK: Duration = Duration::from_millis(50);

/// What the App asks of a connected host.
#[derive(Debug)]
pub enum RemoteRequest {
    /// Start a session.
    Open {
        /// Correlates the answer.
        request_id: String,
        /// A harness id this host advertised.
        harness_id: String,
        /// Where to start it; empty means the host's own default.
        workspace: String,
        /// The pane's width.
        cols: u16,
        /// The pane's height.
        rows: u16,
        /// The name the operator gave it.
        name: Option<String>,
    },
    /// Stream this session's screen, replacing whatever was streaming.
    Watch(String),
    /// Keystrokes for the watched session.
    Input(Vec<u8>),
    /// The pane's geometry changed.
    Resize {
        /// New width.
        cols: u16,
        /// New height.
        rows: u16,
    },
    /// End a session.
    Close(String),
}

/// What a connected host tells the App.
#[derive(Debug)]
pub enum RemoteUpdate {
    /// The host answered and this is what it can do.
    Connected(Box<RemoteCapabilities>),
    /// The connection could not be made, and this is why.
    Failed(String),
    /// The host's session list changed.
    Sessions(Vec<RemoteSessionRow>),
    /// A session started, in answer to an [`RemoteRequest::Open`].
    Opened {
        /// Echoes the request.
        request_id: String,
        /// The new session's id, unprefixed.
        session_id: String,
    },
    /// The host refused to start a session, and this is why.
    OpenFailed(String),
    /// The watched session's screen moved.
    Screen {
        /// Which session, unprefixed.
        session_id: String,
        /// The screen, ready for the ordinary renderer.
        snapshot: Box<ScreenSnapshot>,
    },
}

/// Bring `section` up and serve it until the request channel closes.
///
/// Every outcome is reported — including the failure to connect at all, which is
/// the one an operator most needs to see, and which carries its reason because
/// an unknown host key, a missing binary and a refused login all read as "could
/// not connect" otherwise and have three different fixes.
pub async fn run(
    host_id: String,
    section: RemoteHostSection,
    state_dir: std::path::PathBuf,
    workspace: String,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<RemoteRequest>,
    updates: tokio::sync::mpsc::UnboundedSender<(String, RemoteUpdate)>,
) {
    let mut host = match RemoteHost::connect(&host_id, &section, &state_dir, &workspace).await {
        Ok(host) => host,
        Err(reason) => {
            let _ = updates.send((host_id, RemoteUpdate::Failed(reason)));
            return;
        }
    };
    if let Err(reason) = host.hello().await {
        let _ = updates.send((host_id, RemoteUpdate::Failed(reason)));
        return;
    }

    let mut announced = false;
    // Set by `PumpEvent::Connected`, which is the host's answer arriving —
    // independent of whether that answer listed anything.
    let mut answered = false;
    let mut last_rows: Vec<RemoteSessionRow> = Vec::new();
    // Keyed by session, not just the snapshot: two idle sessions commonly render
    // an identical grid (two fresh shells at the same prompt), so comparing
    // snapshots alone can suppress the first frame of a session just switched to
    // — the pane has already cleared its cached screen, but this cache still
    // holds the previous session's, and an equal grid looks like "nothing
    // changed".
    let mut last_screen: Option<(String, ScreenSnapshot)> = None;

    loop {
        // Requests first, so a keystroke is not held behind a tick.
        while let Ok(request) = requests.try_recv() {
            let _ = serve(&mut host, request, &host_id, &updates).await;
        }
        for event in host.pump().await {
            // The request an event answers is not carried on the wire past the
            // host — only one `Open` is ever outstanding per host at a time on
            // this side, so the id is reported and the App matches it to
            // whatever it was waiting for.
            let update = match event {
                // The answer to `Hello`, and the *only* proof it arrived. A
                // reachable host with no coding CLI and no shell answers with an
                // empty harness list, so inferring arrival from a non-empty one
                // left such a host `Connecting` forever rather than reaching the
                // picker's "offers nothing to start".
                super::PumpEvent::Connected => {
                    answered = true;
                    continue;
                }
                super::PumpEvent::Opened(session_id) => RemoteUpdate::Opened {
                    request_id: String::new(),
                    session_id,
                },
                super::PumpEvent::OpenFailed(reason) => RemoteUpdate::OpenFailed(reason),
            };
            let _ = updates.send((host_id.clone(), update));
        }

        if answered && !announced {
            announced = true;
            let _ = updates.send((
                host_id.clone(),
                RemoteUpdate::Connected(Box::new(host.capabilities().clone())),
            ));
        }
        if host.rows() != last_rows.as_slice() {
            last_rows = host.rows().to_vec();
            let _ = updates.send((host_id.clone(), RemoteUpdate::Sessions(last_rows.clone())));
        }
        // Only when it moved: a watched-but-idle session should cost nothing,
        // which is the whole reason the screen is synchronised state rather than
        // a stream.
        if let Some(update) = next_screen_update(&mut last_screen, host.watching(), host.screen()) {
            let _ = updates.send((
                host_id.clone(),
                RemoteUpdate::Screen {
                    session_id: update.0,
                    snapshot: Box::new(update.1),
                },
            ));
        }

        if requests.is_closed() && requests.is_empty() {
            return;
        }
        tokio::time::sleep(TICK).await;
    }
}

/// Whether the watched screen changed since the last publish, and if so, what
/// to publish.
///
/// Kept separate from [`run`] so the one property that matters — a session
/// switch is never mistaken for "nothing changed" just because the new
/// session's grid happens to equal the old one's — is a plain unit test rather
/// than something only a full SSH-backed integration test could exercise.
///
/// `watching`/`screen` being `None` (nothing watched yet, or no frame has
/// arrived) clears the cache: a `Watch` that lands moments later must be
/// compared against nothing, not the previous session's grid.
fn next_screen_update(
    last_screen: &mut Option<(String, ScreenSnapshot)>,
    watching: Option<&str>,
    screen: Option<ScreenSnapshot>,
) -> Option<(String, ScreenSnapshot)> {
    let (session_id, snapshot) = match (watching, screen) {
        (Some(session_id), Some(snapshot)) => (session_id, snapshot),
        _ => {
            *last_screen = None;
            return None;
        }
    };
    let unchanged = last_screen
        .as_ref()
        .is_some_and(|(id, cached)| id == session_id && cached == &snapshot);
    if unchanged {
        return None;
    }
    *last_screen = Some((session_id.to_string(), snapshot.clone()));
    Some((session_id.to_string(), snapshot))
}

/// Carry out one request.
async fn serve(
    host: &mut RemoteHost,
    request: RemoteRequest,
    host_id: &str,
    updates: &tokio::sync::mpsc::UnboundedSender<(String, RemoteUpdate)>,
) -> Result<(), String> {
    let result = match request {
        RemoteRequest::Open {
            request_id,
            harness_id,
            workspace,
            cols,
            rows,
            name,
        } => {
            host.open(&request_id, &harness_id, &workspace, cols, rows, name)
                .await
        }
        RemoteRequest::Watch(session_id) => host.watch(&session_id).await,
        RemoteRequest::Input(bytes) => host.input(&bytes).await,
        RemoteRequest::Resize { cols, rows } => host.resize(cols, rows).await,
        RemoteRequest::Close(session_id) => host.close(&session_id).await,
    };
    if let Err(reason) = &result {
        // Surfaced rather than swallowed: a request that silently does nothing
        // leaves the operator waiting for a pane that is never coming.
        let _ = updates.send((host_id.to_string(), RemoteUpdate::Failed(reason.clone())));
    }
    result
}

/// The client's node id for a host, minted on first use.
///
/// Exposed so a caller can name the identity directory before the task starts.
pub fn client_state_dir(home: &std::path::Path, host_id: &str) -> std::path::PathBuf {
    home.join("remote").join("clients").join(host_id)
}

/// Unused today, kept honest: the node id a state directory holds.
pub fn client_node_id(dir: &std::path::Path) -> Option<NodeId> {
    medulla_link::keys::read_node_state(&medulla_link::keys::node_path(dir))
        .ok()
        .map(|state| state.node_id)
}

#[cfg(test)]
#[path = "task_tests.rs"]
mod tests;
