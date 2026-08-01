//! Tests for the control-plane gate.

use super::*;

/// An environment map from pairs.
fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A config with fleet tools on and a socket under `root`.
fn config(root: &std::path::Path, enabled: bool) -> medulla::config::TuiConfig {
    let mut config = medulla::config::TuiConfig::default();
    config.mcp.fleet_tools = enabled;
    config.mcp.socket_path = Some(root.join("control.sock").to_string_lossy().into_owned());
    config
}

#[tokio::test]
async fn fleet_tools_off_binds_nothing_at_all() {
    // The switch has to mean no socket, not a socket nobody is granted: an
    // operator who turned this off should be able to see that nothing listens.
    let root = tempfile::tempdir().unwrap();
    let logs = medulla_tui::log::LogBuffer::new();

    let server = start(
        &env(&[]),
        &config(root.path(), false),
        HubSlot::default(),
        &logs,
    )
    .await;

    assert!(server.is_none());
    assert!(!root.path().join("control.sock").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn binding_serves_and_cleans_up_after_itself() {
    let root = tempfile::tempdir().unwrap();
    let logs = medulla_tui::log::LogBuffer::new();
    let path = root.path().join("control.sock");

    let server = start(
        &env(&[]),
        &config(root.path(), true),
        HubSlot::default(),
        &logs,
    )
    .await
    .expect("the socket should bind");
    assert_eq!(server.path(), path);
    assert!(path.exists());

    drop(server);
    // The accept task unwinds asynchronously; give it a moment before asserting
    // the file is gone.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_second_instance_does_not_take_a_live_address() {
    // First binder wins. The loser runs without a control plane rather than
    // silently multiplexing two fleets onto one socket, which would make "where
    // did my task go" unanswerable.
    let root = tempfile::tempdir().unwrap();
    let logs = medulla_tui::log::LogBuffer::new();

    let _first = start(
        &env(&[]),
        &config(root.path(), true),
        HubSlot::default(),
        &logs,
    )
    .await
    .expect("the first binds");

    let second = start(
        &env(&[]),
        &config(root.path(), true),
        HubSlot::default(),
        &logs,
    )
    .await;

    assert!(second.is_none());
    assert!(
        logs.lines()
            .iter()
            .any(|line| line.text.contains("not bound")),
        "the operator should be told why, in the log rather than on the screen"
    );
}
