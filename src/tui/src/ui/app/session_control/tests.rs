//! Tests for the session picker's host step.
//!
//! The assertion that matters most is the *absence* of a change: an operator
//! with no `[[remoteHosts]]` must not gain a keystroke, a row, or a different
//! Escape. That is asserted rather than eyeballed, because it is exactly the
//! kind of regression a feature like this introduces quietly.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::config::{LoadedConfig, RemoteHostSection};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;

use super::super::types::{App, SessionPickerStep};

/// An app with `hosts` configured as remote machines, and a device that hosts.
///
/// `set_local_sessions` is not optional scaffolding: without it
/// `start_session_command` bails with "this device is not hosting" and the
/// picker never opens, so every assertion about it would pass by never running.
/// A shell is offered, which is enough for the picker to have a row.
fn app_with_remote_hosts(hosts: &[&str]) -> App {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.link = Some(medulla::config::LinkConfig::default());
    loaded.config.remote_hosts = hosts
        .iter()
        .map(|host| RemoteHostSection {
            host: host.to_string(),
            enabled: true,
            ..RemoteHostSection::default()
        })
        .collect();
    let mut app = App::new(rt, loaded);
    app.set_local_sessions(local_sessions());
    app
}

/// A `LocalSessions` offering `/bin/sh` and nothing else.
fn local_sessions() -> crate::ui::harness_pane::LocalSessions {
    use std::collections::HashMap;
    let config = medulla::daemon::DaemonConfig {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        providers: Vec::new(),
        default_provider: medulla::protocol::HarnessProvider::Shell,
        workspace: "/tmp".to_string(),
        accessible_dirs: Vec::new(),
        env: HashMap::new(),
        task_timeout_ms: 1_000,
        capability_timeout_ms: None,
        concurrency: 1,
        status_throttle_ms: 1_000,
        max_pending: 1,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        router: None,
        custom_harnesses: Vec::new(),
        budget: None,
        attribution: true,
    };
    let run_task: medulla::daemon::providers::RunTaskFn =
        Arc::new(|_| Box::pin(async { Err("unused".to_string()) }));
    let send: medulla::daemon::SendFn = Arc::new(|_, _| Box::pin(async {}));
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("MEDULLA_SHELL_BIN".to_string(), "/bin/sh".to_string());
    crate::ui::harness_pane::LocalSessions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
        sessions: crate::worker::pty::PtyManager::new(),
        runtimes: Arc::new(std::sync::Mutex::new(vec![
            medulla::daemon::DaemonRuntime::new(config, run_task, send),
        ])),
        hub_address: "medulla-orchestrator".to_string(),
        env,
        workspace: "/tmp".to_string(),
        providers: Vec::new(),
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn with_no_remote_hosts_there_is_exactly_one_host_and_no_host_step() {
    // The ordinary case, and the one that must not change. One host means the
    // picker opens on the harness step exactly as it always has.
    let app = app_with_remote_hosts(&[]);
    let hosts = app.picker_hosts();
    assert_eq!(hosts.len(), 1);
    assert!(!hosts[0].remote);
    assert_eq!(hosts[0].label, "this device");
}

#[test]
fn a_configured_remote_host_is_offered_after_this_device() {
    // This device leads because it is where a session goes when nobody says
    // otherwise, and the first row is what Enter takes.
    let app = app_with_remote_hosts(&["tower.local"]);
    let hosts = app.picker_hosts();
    assert_eq!(hosts.len(), 2);
    assert!(!hosts[0].remote);
    assert!(hosts[1].remote);
    assert_eq!(hosts[1].id, "tower-local");
}

#[test]
fn a_half_written_remote_host_is_not_offered() {
    // An entry with no `host` cannot be dialled, so a row for it could only
    // fail when clicked.
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.remote_hosts = vec![RemoteHostSection {
        name: "half".to_string(),
        enabled: true,
        ..RemoteHostSection::default()
    }];
    let mut app = App::new(rt, loaded);
    // Hosting, so this device is a row — otherwise the assertion below would
    // pass for the wrong reason, with nothing offered at all.
    app.set_local_sessions(local_sessions());
    let hosts = app.picker_hosts();
    assert_eq!(hosts.len(), 1, "only this device is offerable");
    assert!(!hosts[0].remote);
}

#[test]
fn escape_on_the_harness_step_cancels_when_there_is_one_host() {
    // Unchanged behaviour, asserted so the host step cannot quietly take it
    // over: with nothing behind the harness step, Escape still means cancel.
    let mut app = app_with_remote_hosts(&[]);
    app.open_session_picker();
    assert!(app.session_picker.is_some(), "the picker must open");
    assert_eq!(
        app.session_picker.as_ref().map(|p| p.step),
        Some(SessionPickerStep::Harness),
        "one host must not produce a host step"
    );
    app.handle_session_picker_key(key(KeyCode::Esc));
    assert!(
        app.session_picker.is_none(),
        "Escape should close the picker"
    );
}

#[test]
fn escape_on_the_harness_step_goes_back_when_a_host_step_ran() {
    let mut app = app_with_remote_hosts(&["tower.local"]);
    app.open_session_picker();
    let picker = app.session_picker.as_ref().expect("the picker must open");
    assert_eq!(
        picker.step,
        SessionPickerStep::Host,
        "two hosts must open on the host step"
    );
    // Walk forward onto the harness step, then back.
    app.handle_session_picker_key(key(KeyCode::Enter));
    assert_eq!(
        app.session_picker.as_ref().map(|p| p.step),
        Some(SessionPickerStep::Harness),
        "choosing this device should reach the harness step"
    );
    app.handle_session_picker_key(key(KeyCode::Esc));
    assert_eq!(
        app.session_picker.as_ref().map(|p| p.step),
        Some(SessionPickerStep::Host),
        "Escape should return to the host step, not close the picker"
    );
}

#[test]
fn the_host_step_moves_with_the_arrows_and_cancels_on_escape() {
    let mut app = app_with_remote_hosts(&["a.local", "b.local"]);
    app.open_session_picker();
    let picker = app.session_picker.as_ref().expect("the picker must open");
    assert_eq!(picker.step, SessionPickerStep::Host);
    assert_eq!(picker.host_index, 0);

    app.handle_session_picker_key(key(KeyCode::Down));
    assert_eq!(app.session_picker.as_ref().unwrap().host_index, 1);
    app.handle_session_picker_key(key(KeyCode::Down));
    app.handle_session_picker_key(key(KeyCode::Down));
    assert_eq!(
        app.session_picker.as_ref().unwrap().host_index,
        2,
        "the cursor must stop at the last host rather than running off it"
    );
    app.handle_session_picker_key(key(KeyCode::Up));
    assert_eq!(app.session_picker.as_ref().unwrap().host_index, 1);

    app.handle_session_picker_key(key(KeyCode::Esc));
    assert!(app.session_picker.is_none());
}

#[test]
fn choosing_a_disconnected_remote_host_dials_it_rather_than_guessing() {
    // A remote host's harness list has to come from the host: this machine has
    // no idea what is installed over there, and inventing rows would break the
    // picker's promise that everything it shows can start.
    let mut app = app_with_remote_hosts(&["tower.local"]);
    app.open_session_picker();
    assert!(app.session_picker.is_some(), "the picker must open");
    app.handle_session_picker_key(key(KeyCode::Down));
    app.handle_session_picker_key(key(KeyCode::Enter));

    assert_eq!(
        app.session_picker.as_ref().map(|p| p.step),
        Some(SessionPickerStep::Host),
        "the picker waits on the host rather than showing an empty harness list"
    );
    assert!(
        matches!(
            app.take_queued_cmd(),
            Some(crate::ui::app::types::Cmd::ConnectRemoteHost { host_id }) if host_id == "tower-local"
        ),
        "picking a remote host should dial it"
    );
}

#[test]
fn a_connected_host_advances_the_picker_it_left_waiting() {
    // The operator pressed Enter and has been waiting on exactly this answer,
    // so making them press it again would ask for a decision already made.
    let mut app = app_with_remote_hosts(&["tower.local"]);
    app.open_session_picker();
    assert!(app.session_picker.is_some(), "the picker must open");
    app.handle_session_picker_key(key(KeyCode::Down));
    app.handle_session_picker_key(key(KeyCode::Enter));
    let _ = app.take_queued_cmd();

    app.remote_host_connected(
        "tower-local",
        medulla::protocol::RemoteCapabilities {
            version: "0.12.0".to_string(),
            harnesses: vec![medulla::protocol::RemoteHarnessChoice {
                id: "shell:zsh".to_string(),
                provider: "shell".to_string(),
                preset: None,
                display_name: "zsh".to_string(),
            }],
            workspace: "/work".to_string(),
            workspaces: Vec::new(),
            host_name: "tower".to_string(),
        },
    );
    app.resume_picker_host("tower-local");

    assert_eq!(
        app.session_picker.as_ref().map(|p| p.step),
        Some(SessionPickerStep::Harness),
        "the picker should have moved on by itself"
    );
    assert_eq!(
        app.session_picker.as_ref().map(|p| p.cwd.as_str()),
        Some("/work"),
        "the workspace step should start from the remote host's directory"
    );
}

#[test]
fn a_host_that_answered_for_a_different_row_does_not_move_the_picker() {
    // The operator has moved on. Advancing the picker under them would be worse
    // than leaving it where they put it.
    let mut app = app_with_remote_hosts(&["a.local", "b.local"]);
    app.open_session_picker();
    assert!(app.session_picker.is_some(), "the picker must open");
    app.handle_session_picker_key(key(KeyCode::Down));
    app.resume_picker_host("b-local");
    assert_eq!(
        app.session_picker.as_ref().map(|p| p.step),
        Some(SessionPickerStep::Host),
        "an answer from a host the cursor is not on must change nothing"
    );
}

#[test]
fn a_remote_workspace_is_taken_on_trust_rather_than_checked_locally() {
    // A remote directory cannot be `stat`ed from here. Checking it against this
    // filesystem answered "no" for every valid remote path, which is how
    // `/work` — a real directory on the host — was refused as "not a directory".
    let mut app = app_with_remote_hosts(&["tower.local"]);
    app.remote_host_connected("tower-local", capabilities_with_shell());
    app.open_session_picker();
    assert!(app.session_picker.is_some(), "the picker must open");

    // Onto the remote host, through to its workspace step.
    app.handle_session_picker_key(key(KeyCode::Down));
    app.handle_session_picker_key(key(KeyCode::Enter));
    app.handle_session_picker_key(key(KeyCode::Enter));
    assert_eq!(
        app.session_picker.as_ref().map(|p| p.step),
        Some(SessionPickerStep::Workspace)
    );

    // A path that certainly does not exist on this machine.
    for ch in "/does/not/exist/here".chars() {
        app.handle_session_picker_key(key(KeyCode::Char(ch)));
    }
    assert_eq!(
        app.selected_picker_workspace_for_test().as_deref(),
        Some("/does/not/exist/here"),
        "a remote path must be accepted without a local existence check"
    );
}

#[test]
fn starting_on_a_remote_host_does_not_fall_back_to_this_device() {
    // The picker was cleared before the launch dispatched, so the question
    // "which machine is this for?" was asked of a picker that no longer existed
    // — and answered "this device". A remote launch became a local one, silently.
    let mut app = app_with_remote_hosts(&["tower.local"]);
    app.remote_host_connected("tower-local", capabilities_with_shell());
    app.open_session_picker();
    app.handle_session_picker_key(key(KeyCode::Down));
    app.handle_session_picker_key(key(KeyCode::Enter));
    app.handle_session_picker_key(key(KeyCode::Enter));
    // Enter on the workspace step launches.
    app.handle_session_picker_key(key(KeyCode::Enter));

    assert!(
        app.session_picker.is_none(),
        "the picker should have closed"
    );
    // The host is not actually connected, so the request cannot be delivered —
    // and that specific complaint is the proof it took the remote path at all.
    // A local launch would have complained about a directory instead.
    let status = app.status().to_string();
    assert!(
        status.contains("tower.local") || status.contains("not connected"),
        "expected a remote-launch outcome, got: {status}"
    );
    assert!(
        !status.contains("is not a directory"),
        "that is the local launch path's complaint: {status}"
    );
}

/// Capabilities advertising a single shell, enough to reach the workspace step.
fn capabilities_with_shell() -> medulla::protocol::RemoteCapabilities {
    medulla::protocol::RemoteCapabilities {
        version: "0.12.0".to_string(),
        harnesses: vec![medulla::protocol::RemoteHarnessChoice {
            id: "bash".to_string(),
            provider: "shell".to_string(),
            preset: None,
            display_name: "bash".to_string(),
        }],
        workspace: "/work".to_string(),
        workspaces: Vec::new(),
        host_name: "tower".to_string(),
    }
}

#[test]
fn remote_hosts_are_reachable_from_a_client_that_hosts_nothing() {
    // `MEDULLA_HOST=0` is a supported configuration, and it is the one where
    // remote hosts are the *only* thing there is to reach. Returning early on a
    // missing local surface made them unreachable — the picker never opened.
    let mut app = app_with_remote_hosts(&["tower.local"]);
    app.clear_local_sessions_for_test();

    app.open_session_picker();
    let picker = app
        .session_picker
        .as_ref()
        .expect("a client with remote hosts must still get a picker");
    assert_eq!(picker.hosts.len(), 1, "this device offers nothing here");
    assert!(picker.hosts[0].remote);
    assert_eq!(
        picker.step,
        SessionPickerStep::Host,
        "a lone remote host still needs dialling, which happens on the host step"
    );
}

#[test]
fn this_device_is_not_offered_when_it_cannot_host() {
    // A row that could only fail when chosen is worse than no row.
    let mut app = app_with_remote_hosts(&["tower.local"]);
    app.clear_local_sessions_for_test();
    let hosts = app.picker_hosts();
    assert!(
        hosts.iter().all(|host| host.remote),
        "a non-hosting device must not offer itself: {hosts:?}"
    );
}

#[test]
fn a_client_with_neither_local_nor_remote_hosts_says_so() {
    let mut app = app_with_remote_hosts(&[]);
    app.clear_local_sessions_for_test();
    app.open_session_picker();
    assert!(app.session_picker.is_none());
    assert!(
        app.status().contains("no [[remoteHosts]]"),
        "the message should name both halves of why: {}",
        app.status()
    );
}

#[test]
fn a_lone_local_host_still_opens_straight_onto_the_harness_step() {
    // The regression guard for the ordinary case, restated against the new
    // branch: adding remote support must not cost a keystroke to someone who
    // has none configured.
    let mut app = app_with_remote_hosts(&[]);
    app.open_session_picker();
    assert_eq!(
        app.session_picker.as_ref().map(|p| p.step),
        Some(SessionPickerStep::Harness),
    );
}
