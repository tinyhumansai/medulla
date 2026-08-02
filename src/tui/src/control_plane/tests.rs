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
        true,
        HubSlot::default(),
        None,
        &logs,
    )
    .await;

    assert!(server.is_none());
    assert!(!root.path().join("control.sock").exists());
}

#[tokio::test]
async fn a_hub_that_did_not_start_gets_no_control_plane() {
    let root = tempfile::tempdir().unwrap();
    let logs = medulla_tui::log::LogBuffer::new();

    let server = start(
        &env(&[]),
        &config(root.path(), true),
        false,
        HubSlot::default(),
        None,
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
        true,
        HubSlot::default(),
        Some("this-device".into()),
        &logs,
    )
    .await
    .expect("the socket should bind");
    assert_eq!(server.path(), path);
    assert!(path.exists());

    drop(server);
    // The accept task unwinds asynchronously. Poll with a generous bound so a
    // loaded CI worker cannot turn scheduler latency into a flaky assertion.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while path.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
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
        true,
        HubSlot::default(),
        Some("this-device".into()),
        &logs,
    )
    .await
    .expect("the first binds");

    let second = start(
        &env(&[]),
        &config(root.path(), true),
        true,
        HubSlot::default(),
        Some("this-device".into()),
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

#[cfg(unix)]
#[tokio::test]
async fn an_environment_socket_path_does_not_bypass_parent_security() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let env_parent = root.path().join("environment-path");
    std::fs::create_dir(&env_parent).unwrap();
    std::fs::set_permissions(&env_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
    let env_socket = env_parent.join("control.sock");
    let env_socket_string = env_socket.to_string_lossy().into_owned();
    let logs = medulla_tui::log::LogBuffer::new();

    let server = start(
        &env(&[(CONTROL_SOCKET_ENV, &env_socket_string)]),
        &config(root.path(), true),
        true,
        HubSlot::default(),
        None,
        &logs,
    )
    .await;

    assert!(server.is_none());
    assert!(!env_socket.exists());
    assert_eq!(
        std::fs::metadata(&env_parent).unwrap().permissions().mode() & 0o777,
        0o777,
        "Medulla must reject, not chmod, an environment-selected parent"
    );
}
