//! Unit tests for the CLI plumbing: subcommand dispatch, the per-subcommand
//! flag parsers, help text, the `sessions` JSON, and the core-socket plan.

use std::collections::HashMap;
use std::path::PathBuf;

use medulla::auth::Provider;
use medulla::tinyplace::HarnessProvider;

use super::*;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[test]
fn dispatches_subcommands() {
    assert_eq!(parse_command(&argv(&[])), Command::Tui);
    assert_eq!(parse_command(&argv(&["daemon", "--foo"])), Command::Daemon);
    assert_eq!(parse_command(&argv(&["version"])), Command::Version);
    assert_eq!(parse_command(&argv(&["-v"])), Command::Version);
    assert_eq!(parse_command(&argv(&["help"])), Command::Help);
    assert_eq!(parse_command(&argv(&["-h"])), Command::Help);
    assert_eq!(parse_command(&argv(&["sessions"])), Command::Sessions);
    assert_eq!(parse_command(&argv(&["login"])), Command::Login);
    assert_eq!(parse_command(&argv(&["logout"])), Command::Logout);
    assert_eq!(
        parse_command(&argv(&["codex", "resume"])),
        Command::Wrapper(HarnessProvider::Codex)
    );
    assert_eq!(
        parse_command(&argv(&["claude"])),
        Command::Wrapper(HarnessProvider::Claude)
    );
    assert_eq!(
        parse_command(&argv(&["opencode", "--foo"])),
        Command::Wrapper(HarnessProvider::Opencode)
    );
    assert_eq!(parse_command(&argv(&["memory", "status"])), Command::Memory);
    assert_eq!(parse_command(&argv(&["update"])), Command::Update);
    assert_eq!(
        parse_command(&argv(&["update", "--check"])),
        Command::Update
    );
    assert_eq!(parse_command(&argv(&["--config", "x.json"])), Command::Tui);
}

#[test]
fn update_args_parse() {
    assert_eq!(parse_update_args(&argv(&[])), UpdateArgs { check: false });
    assert_eq!(
        parse_update_args(&argv(&["--check"])),
        UpdateArgs { check: true }
    );
    // Unknown flags are ignored.
    assert_eq!(
        parse_update_args(&argv(&["--check", "--force"])),
        UpdateArgs { check: true }
    );
}

#[test]
fn memory_args_parse() {
    // Simple actions.
    assert_eq!(
        parse_memory_args(&argv(&["status"])).unwrap().action,
        MemoryAction::Status
    );
    assert_eq!(
        parse_memory_args(&argv(&["backfill"])).unwrap().action,
        MemoryAction::Backfill
    );
    // Search joins the query words and honors flags.
    let a = parse_memory_args(&argv(&[
        "search", "how", "to", "test", "--facet", "stack", "--k", "3", "--json", "--config",
        "c.toml",
    ]))
    .unwrap();
    assert_eq!(a.action, MemoryAction::Search("how to test".into()));
    assert_eq!(a.facet.as_deref(), Some("stack"));
    assert_eq!(a.k, 3);
    assert!(a.json);
    assert_eq!(a.config.as_deref(), Some("c.toml"));
    // Errors: no action, empty search, unknown action, bad --k.
    assert!(parse_memory_args(&argv(&[])).is_err());
    assert!(parse_memory_args(&argv(&["search"])).is_err());
    assert!(parse_memory_args(&argv(&["frobnicate"])).is_err());
    assert!(parse_memory_args(&argv(&["search", "q", "--k", "nan"])).is_err());
}

#[test]
fn parses_tui_flags() {
    assert_eq!(parse_tui_args(&argv(&[])), TuiArgs::default());
    let a = parse_tui_args(&argv(&["--config", "c.json", "--core", "--no-alt-screen"]));
    assert_eq!(
        a,
        TuiArgs {
            config: Some("c.json".into()),
            alt_screen: false,
            core: true
        }
    );
    // A dangling --config keeps the default (None → layered discovery).
    assert_eq!(parse_tui_args(&argv(&["--config"])).config, None);
}

#[test]
fn help_names_the_binary() {
    let text = help_text();
    assert!(text.starts_with("medulla "));
    assert!(text.contains("--no-alt-screen"));
}

#[test]
fn resolve_prefers_override_then_xdg_then_state() {
    let over = resolve_socket_path(Some("/tmp/x.sock"), Some("/run/user/1000"), Some("/state"));
    assert_eq!(over.unwrap(), PathBuf::from("/tmp/x.sock"));

    let xdg = resolve_socket_path(None, Some("/run/user/1000"), Some("/state"));
    assert_eq!(
        xdg.unwrap(),
        PathBuf::from("/run/user/1000/medulla/core.sock")
    );

    let state = resolve_socket_path(None, None, Some("/state"));
    assert_eq!(state.unwrap(), PathBuf::from("/state/core.sock"));

    assert!(resolve_socket_path(None, None, None).is_none());
    // Empty strings are treated as unset.
    assert!(resolve_socket_path(Some(""), None, None).is_none());
}

#[test]
fn core_plan_skips_when_not_wanted() {
    let plan = core_socket_plan(false, None, None, None, |_| true);
    assert_eq!(plan, CorePlan::Skip);
}

#[test]
fn core_plan_connects_when_socket_present() {
    let plan = core_socket_plan(true, Some("/run/core.sock"), None, None, |_| true);
    assert_eq!(plan, CorePlan::Connect(PathBuf::from("/run/core.sock")));
}

#[test]
fn core_plan_falls_back_when_absent() {
    let plan = core_socket_plan(true, Some("/run/core.sock"), None, None, |_| false);
    match plan {
        CorePlan::Fallback(note) => assert!(note.contains("not present")),
        other => panic!("expected fallback, got {other:?}"),
    }
    let plan = core_socket_plan(true, None, None, None, |_| false);
    match plan {
        CorePlan::Fallback(note) => assert!(note.contains("no core socket resolved")),
        other => panic!("expected fallback, got {other:?}"),
    }
}

#[test]
fn login_args_parse() {
    assert_eq!(parse_login_args(&argv(&[])).unwrap(), LoginArgs::default());
    let a = parse_login_args(&argv(&[
        "--provider",
        "github",
        "--no-browser",
        "--token",
        "deadbeef",
        "--config",
        "c.json",
    ]))
    .unwrap();
    assert_eq!(a.provider, Provider::Github);
    assert!(a.no_browser);
    assert_eq!(a.token.as_deref(), Some("deadbeef"));
    assert_eq!(a.config.as_deref(), Some("c.json"));
    // Unknown provider is a friendly error.
    assert!(parse_login_args(&argv(&["--provider", "myspace"])).is_err());
}

#[test]
fn help_text_carries_crate_version() {
    let text = help_text();
    assert!(text.contains(env!("CARGO_PKG_VERSION")));
    assert!(text.contains("medulla daemon"));
    assert!(text.contains("medulla login"));
    assert!(text.contains("medulla codex"));
    assert!(text.contains("--no-bridge"));
    assert!(text.contains("--provider"));
    assert!(text.contains("--core"));
}

#[test]
fn sessions_json_is_valid_json_array() {
    // Point the scan dirs at an empty temp path so the result is deterministic
    // ([]), independent of the developer's real ~/.claude / ~/.codex history.
    let tmp = std::env::temp_dir().join(format!("medulla-cli-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".to_string(),
        tmp.join("claude").to_string_lossy().into_owned(),
    );
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
        tmp.join("codex").to_string_lossy().into_owned(),
    );
    let json = sessions_json(&env, tmp.to_str().unwrap()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());
    assert_eq!(parsed.as_array().unwrap().len(), 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn tui_args_default_and_debug() {
    let d = TuiArgs::default();
    assert_eq!(d.config, None);
    assert!(d.alt_screen);
    assert!(!d.core);
    // Command derives Debug/Eq for assertions.
    assert!(format!("{:?}", Command::Tui).contains("Tui"));
    assert_ne!(Command::Tui, Command::Daemon);
}

#[test]
fn core_plan_connects_via_state_dir_when_no_config_socket() {
    // resolve_socket_path falls back to <stateDir>/core.sock.
    let plan = core_socket_plan(true, None, None, Some("/var/lib/medulla"), |_| true);
    match plan {
        CorePlan::Connect(path) => {
            assert!(path.ends_with("core.sock"));
        }
        other => panic!("expected connect, got {other:?}"),
    }
}
