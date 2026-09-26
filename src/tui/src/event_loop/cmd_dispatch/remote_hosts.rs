//! Bringing a `[[remoteHosts]]` machine up, off the UI thread.
//!
//! The connection is made out here because making one means running `ssh` and
//! waiting on it — a login prompt, a `ProxyJump`, a host that is simply not
//! answering. None of that may happen on the render path.
//!
//! What is started is a *task*, not a request: it owns the link for as long as
//! the host is in use, and every session on that machine is multiplexed over it.
//! The App talks to it through the channel returned here.

use medulla::config::TuiConfig;

use super::AppMsg;
use medulla_tui::remote::client::{task, RemoteRequest, RemoteUpdate};

/// Dial `host_id` and keep serving it until the App drops its request channel.
///
/// Returns the sender the App holds on to. `None` when the id names no
/// configured host, which is reported through `msg_tx` rather than silently —
/// it means config and UI disagree, and that is worth seeing.
pub(super) fn spawn_connect(
    host_id: String,
    config: &TuiConfig,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) -> Option<tokio::sync::mpsc::UnboundedSender<RemoteRequest>> {
    // Resolved through the same filter the picker offered from. A raw scan would
    // ignore `enabled` and the id dedupe, so a disabled entry above an enabled
    // one with the same slug would win here — dialling a machine the operator
    // was never shown.
    let Some(section) =
        medulla::config::remote_host_section(&config.remote_hosts, &host_id).cloned()
    else {
        let _ = msg_tx.send(AppMsg::RemoteHostFailed {
            host_id: host_id.clone(),
            reason: format!("no [[remoteHosts]] entry called {host_id}"),
        });
        return None;
    };

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let home = medulla::home::medulla_home(&env);
    // One identity directory per host: the link takes an advisory lock on it for
    // its lifetime, so hosts sharing a directory would leave all but the first
    // unable to connect.
    let state_dir = task::client_state_dir(&home, &host_id);
    let workspace = section.workspace.clone();

    let (request_tx, request_rx) = tokio::sync::mpsc::unbounded_channel();
    let (update_tx, mut update_rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(task::run(
        host_id.clone(),
        section,
        state_dir,
        workspace,
        request_rx,
        update_tx,
    ));

    // Translate the task's vocabulary into the App's. Two channels rather than
    // one because the task speaks only about its own host and should not have to
    // know what an `AppMsg` is.
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        while let Some((host_id, update)) = update_rx.recv().await {
            let message = match update {
                RemoteUpdate::Connected(capabilities) => AppMsg::RemoteHostConnected {
                    host_id,
                    capabilities,
                },
                RemoteUpdate::Failed(reason) => AppMsg::RemoteHostFailed { host_id, reason },
                RemoteUpdate::Sessions(rows) => AppMsg::RemoteHostSessions { host_id, rows },
                RemoteUpdate::Opened { session_id, .. } => AppMsg::RemoteSessionOpened {
                    host_id,
                    session_id,
                },
                RemoteUpdate::OpenFailed(reason) => AppMsg::RemoteOpenFailed { host_id, reason },
                RemoteUpdate::Screen {
                    session_id,
                    snapshot,
                } => AppMsg::RemoteScreen {
                    host_id,
                    session_id,
                    snapshot,
                },
            };
            if tx.send(message).is_err() {
                return;
            }
        }
    });

    Some(request_tx)
}
