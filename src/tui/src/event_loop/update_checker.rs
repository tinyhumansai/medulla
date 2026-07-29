//! Periodic release-update checks spawned by the interactive event loop.

use std::time::Duration;

use super::types::AppMsg;

/// Spawn the release checker unless config or environment disables it.
///
/// The first probe waits roughly ten seconds so startup work wins; later probes
/// run every six hours. A dropped receiver ends the background task.
pub(super) fn spawn_update_checker(
    loaded: &medulla::config::LoadedConfig,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    if !loaded.config.update.enabled(&env) {
        return;
    }
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        let url = medulla::update::update_url();
        let current = env!("CARGO_PKG_VERSION");
        let mut first = true;
        loop {
            let delay = if first {
                Duration::from_secs(10)
            } else {
                Duration::from_secs(6 * 60 * 60)
            };
            first = false;
            tokio::time::sleep(delay).await;
            if let Ok(Some(info)) = medulla::update::check_for_update(&url, current).await {
                let notice = format!("update v{} available — run `medulla update`", info.version);
                if tx.send(AppMsg::UpdateAvailable(notice)).is_err() {
                    break;
                }
            }
        }
    });
}
