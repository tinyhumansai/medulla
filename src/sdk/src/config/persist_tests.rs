//! Unit tests for onboarding-state persistence ([`super::persist`]).
//!
//! Split out of the main config tests to keep both files under the repository's
//! 500-line ceiling.

use super::*;

#[test]
fn persists_the_welcome_flag_to_a_new_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join("config.toml");

    super::persist_welcome_completed(&path, true).expect("persist should succeed");

    let text = std::fs::read_to_string(&path).expect("file should exist");
    let parsed: TuiConfig = toml::from_str(&text).expect("should reparse");
    assert!(parsed.onboarding.welcome_completed);
}

#[test]
fn persisting_the_welcome_flag_preserves_unrelated_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "stateDir = \"/tmp/keep-me\"\n\n[theme]\nprimary = \"#ff0000\"\n",
    )
    .expect("seed config");

    super::persist_welcome_completed(&path, true).expect("persist should succeed");

    let text = std::fs::read_to_string(&path).expect("read back");
    let parsed: TuiConfig = toml::from_str(&text).expect("should reparse");
    assert!(parsed.onboarding.welcome_completed);
    assert_eq!(parsed.state_dir, "/tmp/keep-me");
    assert_eq!(parsed.theme.primary.as_deref(), Some("#ff0000"));
}

#[test]
fn the_welcome_flag_can_be_cleared_to_replay_onboarding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    super::persist_welcome_completed(&path, true).expect("persist true");
    super::persist_welcome_completed(&path, false).expect("persist false");

    let text = std::fs::read_to_string(&path).expect("read back");
    let parsed: TuiConfig = toml::from_str(&text).expect("should reparse");
    assert!(!parsed.onboarding.welcome_completed);
}

#[test]
fn welcome_flag_defaults_to_false_when_absent() {
    let parsed: TuiConfig = toml::from_str("stateDir = \"/tmp/x\"\n").expect("parse");
    assert!(!parsed.onboarding.welcome_completed);
}

#[test]
fn persisting_over_an_unparseable_config_errors_rather_than_clobbering_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "this is not = = valid toml [[[").expect("seed");

    let err = super::persist_welcome_completed(&path, true)
        .expect_err("an unparseable config must not be silently overwritten");

    assert!(err.to_string().contains("Cannot parse"), "got: {err}");
    // The original bytes survive, so the user can fix them by hand.
    assert!(std::fs::read_to_string(&path)
        .expect("still readable")
        .contains("not = = valid"));
}

#[test]
fn persisting_under_a_file_masquerading_as_a_directory_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "i am a file").expect("seed");

    // `blocker` is a file, so treating it as a parent directory fails. Which
    // syscall reports it is platform-dependent: unix surfaces ENOTDIR from the
    // read (naming the full path), Windows fails at create_dir_all (naming the
    // parent). Assert only what must hold everywhere — a clean error naming the
    // offending path, never a panic.
    let target = blocker.join("config.toml");
    let err =
        super::persist_welcome_completed(&target, true).expect_err("cannot write under a file");

    let message = err.to_string();
    assert!(
        message.contains("Cannot read") || message.contains("Cannot create"),
        "got: {message}"
    );
    assert!(
        message.contains("blocker"),
        "should name the offending path: {message}"
    );
}

#[test]
fn persisting_merges_into_an_existing_onboarding_section() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[onboarding]\nwelcomeCompleted = false\nsomeFutureKey = \"keep me\"\n",
    )
    .expect("seed");

    super::persist_welcome_completed(&path, true).expect("persist");

    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(text.contains("welcomeCompleted = true"));
    assert!(
        text.contains("keep me"),
        "unrelated onboarding keys must survive: {text}"
    );
}

#[test]
fn persisting_replaces_a_non_table_onboarding_value() {
    // A hand-edited config could set `onboarding` to a scalar; writing the flag
    // must still succeed rather than panicking on the unexpected shape.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "onboarding = \"nonsense\"\n").expect("seed");

    super::persist_welcome_completed(&path, true).expect("persist");

    let parsed: TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert!(parsed.onboarding.welcome_completed);
}

#[test]
fn persist_setting_creates_and_merges_sections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join("config.toml");

    // Writing into a file that does not exist yet creates it and its parent.
    super::persist_setting(&path, "update", "check", toml::Value::Boolean(false)).expect("write");
    // A second key in the same section merges rather than replacing.
    super::persist_setting(&path, "medulla", "maxPasses", toml::Value::Integer(7)).expect("write");

    let parsed: TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert!(!parsed.update.check);
    assert_eq!(parsed.medulla.max_passes, Some(7));
}

#[test]
fn persist_setting_preserves_unrelated_sections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[theme]\nprimary = \"cyan\"\n\n[tinyplace]\nhandle = \"me\"\n",
    )
    .expect("seed");

    super::persist_setting(
        &path,
        "tinyplace",
        "autoDiscoverPeers",
        toml::Value::Boolean(false),
    )
    .expect("write");

    let parsed: TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert_eq!(parsed.theme.primary.as_deref(), Some("cyan"));
    let tinyplace = parsed.tinyplace.expect("tinyplace section");
    assert!(!tinyplace.auto_discover_peers);
    assert_eq!(
        tinyplace.handle.as_deref(),
        Some("me"),
        "sibling key survives"
    );
}

#[test]
fn persist_setting_preserves_json_format_and_unrelated_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("medulla.tui.json");
    std::fs::write(
        &path,
        r#"{
  "theme": {"primary": "cyan"},
  "harness": {"skipPermissions": true}
}"#,
    )
    .expect("seed");

    super::persist_setting(
        &path,
        "harness",
        "recentWorkspaces",
        toml::Value::Array(vec![toml::Value::String("/work/medulla".into())]),
    )
    .expect("write");

    let saved = std::fs::read_to_string(&path).expect("read");
    let parsed: serde_json::Value = serde_json::from_str(&saved).expect("JSON remains valid");
    assert_eq!(parsed["theme"]["primary"], "cyan");
    assert_eq!(parsed["harness"]["skipPermissions"], true);
    assert_eq!(
        parsed["harness"]["recentWorkspaces"],
        serde_json::json!(["/work/medulla"])
    );
}

#[test]
fn persist_setting_uses_json_for_extensionless_config_like_the_loader() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("medulla-config");
    std::fs::write(&path, r#"{"harness":{"skipPermissions":true}}"#).expect("seed");

    super::persist_setting(
        &path,
        "harness",
        "recentWorkspaces",
        toml::Value::Array(vec![toml::Value::String("/work/medulla".into())]),
    )
    .expect("write");

    let saved = std::fs::read_to_string(&path).expect("read");
    let parsed: serde_json::Value = serde_json::from_str(&saved).expect("JSON remains valid");
    assert_eq!(parsed["harness"]["skipPermissions"], true);
    assert_eq!(
        parsed["harness"]["recentWorkspaces"],
        serde_json::json!(["/work/medulla"])
    );
}

#[test]
fn persist_section_replaces_a_complete_json_section_and_preserves_others() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("medulla.tui.json");
    std::fs::write(
        &path,
        r#"{"theme":{"primary":"cyan"},"statusLine":{"path":"hidden"}}"#,
    )
    .expect("seed");
    let mut status_line = toml::Table::new();
    status_line.insert("state".into(), toml::Value::String("line2".into()));
    status_line.insert("path".into(), toml::Value::String("line1".into()));

    super::persist_section(&path, "statusLine", status_line).expect("write");

    let saved = std::fs::read_to_string(&path).expect("read");
    let parsed: serde_json::Value = serde_json::from_str(&saved).expect("JSON remains valid");
    assert_eq!(parsed["theme"]["primary"], "cyan");
    assert_eq!(parsed["statusLine"]["state"], "line2");
    assert_eq!(parsed["statusLine"]["path"], "line1");
}

#[test]
fn subscription_routing_strategy_persists_without_clobbering_host_strategy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "routingStrategy = \"cpuFirst\"\n[onboarding]\nwelcomeCompleted = true\n",
    )
    .unwrap();

    super::persist_subscription_routing_strategy(&path, "mostAvailableBudget").unwrap();

    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("routingStrategy = \"cpuFirst\""), "{saved}");
    assert!(
        saved.contains("subscriptionRoutingStrategy = \"mostAvailableBudget\""),
        "{saved}"
    );
    assert!(saved.contains("welcomeCompleted = true"), "{saved}");
}

#[test]
fn persist_section_replaces_only_the_named_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[theme]\nprimary = \"red\"\nstale = true\n\n[memory]\nenabled = true\n",
    )
    .unwrap();
    let mut theme = toml::Table::new();
    theme.insert("primary".into(), toml::Value::String("cyan".into()));

    super::persist_section(&path, "theme", theme).unwrap();

    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(saved["theme"]["primary"].as_str(), Some("cyan"));
    assert!(saved["theme"].get("stale").is_none());
    assert_eq!(saved["memory"]["enabled"].as_bool(), Some(true));
}

#[test]
fn clear_setting_removes_only_its_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[medulla]\nmaxPasses = 9\nmaxSteps = 40\n").expect("seed");

    super::clear_setting(&path, "medulla", "maxPasses").expect("clear");

    let parsed: TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert_eq!(parsed.medulla.max_passes, None, "cleared back to unset");
    assert_eq!(parsed.medulla.max_steps, Some(40));
}

#[test]
fn clear_setting_on_a_missing_file_is_a_no_op() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("absent.toml");
    super::clear_setting(&path, "medulla", "maxPasses").expect("no-op");
    assert!(!path.exists(), "clearing must not create the file");
}

#[test]
fn the_hub_roster_round_trips_through_the_config_file() {
    // The whole point: a worker added in the Workers tab must still be there on
    // the next launch. It used to live only in memory, seeded from the
    // environment at boot, so the tab was empty every time however many peers
    // were reachable.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[onboarding]\nwelcomeCompleted = true\n").expect("seed");

    let workers = vec![
        crate::config::HubWorkerConfig {
            roles: Vec::new(),
            id: "alpha".into(),
            address: "3Hob1FxUwsy1K2rweppbmCkuPef6unAr5Amj6kQ2fM3A".into(),
            harness: "claude".into(),
            label: Some("laptop".into()),
            selected: true,
        },
        crate::config::HubWorkerConfig {
            roles: Vec::new(),
            id: "beta".into(),
            address: "@someone".into(),
            harness: "codex".into(),
            label: None,
            selected: false,
        },
    ];
    super::persist_hub_workers(&path, &workers).expect("write");

    let parsed: crate::config::TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert_eq!(
        parsed.hub.workers, workers,
        "the roster must survive a save"
    );
    assert!(
        parsed.onboarding.welcome_completed,
        "an unrelated section must not be trampled"
    );
}

#[test]
fn removing_the_last_worker_is_remembered_as_removal() {
    // A merge would resurrect what the operator just deleted, so the list is
    // replaced wholesale — including replacing it with nothing.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    super::persist_hub_workers(
        &path,
        &[crate::config::HubWorkerConfig {
            roles: Vec::new(),
            id: "alpha".into(),
            address: "addr".into(),
            harness: "claude".into(),
            label: None,
            selected: false,
        }],
    )
    .expect("write");
    super::persist_hub_workers(&path, &[]).expect("write empty");

    let parsed: crate::config::TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert!(
        parsed.hub.workers.is_empty(),
        "got {:?}",
        parsed.hub.workers
    );
}

#[test]
fn daemon_workspace_allowlist_replaces_only_workflow_workspaces() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[workflow]\nmaxLanes = 8\nworkspaces = [\"/old\"]\n\n[theme]\nprimary = \"cyan\"\n",
    )
    .unwrap();

    super::persist_workflow_workspaces(&path, &["/one".into(), "/two".into()]).unwrap();

    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(saved["workflow"]["maxLanes"].as_integer(), Some(8));
    assert_eq!(saved["theme"]["primary"].as_str(), Some("cyan"));
    assert_eq!(
        saved["workflow"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["/one", "/two"]
    );
}

#[test]
fn custom_harnesses_persist_without_secret_values_or_unrelated_loss() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[backend]\nbaseUrl = \"https://example.test\"\n").unwrap();
    let preset = crate::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek via Claude | claude | deepseek/model | deepseek/fast | this-device",
    )
    .unwrap();

    super::persist_custom_harnesses(&path, &[preset]).unwrap();

    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("https://example.test"));
    assert!(text.contains("deepseek/model"));
    assert!(text.contains("OPENROUTER_API_KEY"));
    assert!(!text.contains("sk-or-"));
}

#[test]
fn daemon_master_roster_persists_public_peer_data_without_identity_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[tinyplace]\nidentityDir = \"/worker-wallet\"\nbaseUrl = \"https://relay\"\n",
    )
    .unwrap();
    let peer = super::Peer {
        id: "master-id".into(),
        name: Some("Primary master".into()),
        handle: Some("@master".into()),
        address: Some("master-id".into()),
        tags: Some(vec!["master".into()]),
        description: Some("Orchestrator".into()),
        protocol: "task".into(),
    };

    super::persist_tinyplace_peers(&path, &[peer]).unwrap();

    let text = std::fs::read_to_string(path).unwrap();
    let saved: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        saved["tinyplace"]["identityDir"].as_str(),
        Some("/worker-wallet")
    );
    assert_eq!(
        saved["tinyplace"]["baseUrl"].as_str(),
        Some("https://relay")
    );
    assert_eq!(
        saved["tinyplace"]["peers"][0]["id"].as_str(),
        Some("master-id")
    );
    assert!(!text.contains("private"));
    assert!(!text.contains("token"));
}

#[test]
fn a_local_host_that_named_no_address_is_written_without_one() {
    // Loading fills `address` from the section default, so persisting the
    // in-memory value verbatim turns "derive one for me" into "bind the
    // primary's address" — and every extra collides on the next launch.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("medulla.tui.json.toml");
    let hosts = vec![
        crate::config::HostSection {
            name: "backend".to_string(),
            workspace: "/tmp/backend".to_string(),
            ..crate::config::HostSection::default()
        },
        crate::config::HostSection {
            address: "chosen-by-hand".to_string(),
            workspace: "/tmp/other".to_string(),
            ..crate::config::HostSection::default()
        },
    ];

    super::persist_local_hosts(&path, &hosts).expect("write");
    let written = std::fs::read_to_string(&path).expect("read back");

    assert!(
        !written.contains(&crate::config::HostSection::default().address),
        "the default address must not be written back: {written}"
    );
    // An address the operator actually picked still survives.
    assert!(written.contains("chosen-by-hand"), "{written}");
    assert!(written.contains("backend"), "{written}");
}

#[test]
fn persisting_hosts_leaves_every_other_section_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cfg.toml");
    std::fs::write(&path, "[onboarding]\nwelcomeCompleted = true\n").expect("seed");

    super::persist_local_hosts(
        &path,
        &[crate::config::HostSection {
            name: "api".to_string(),
            workspace: "/tmp/api".to_string(),
            ..crate::config::HostSection::default()
        }],
    )
    .expect("write");

    let written = std::fs::read_to_string(&path).expect("read back");
    assert!(written.contains("welcomeCompleted"), "{written}");
    assert!(written.contains("[[hosts]]"), "{written}");
}
