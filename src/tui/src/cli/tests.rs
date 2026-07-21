//! Unit tests for the CLI plumbing: subcommand dispatch, the per-subcommand
//! flag parsers, help text, and the `sessions` JSON.

use std::collections::HashMap;

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
    assert_eq!(parse_command(&argv(&["init"])), Command::Init);
    assert_eq!(parse_command(&argv(&["init", "some/dir"])), Command::Init);
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
    let a = parse_tui_args(&argv(&["--config", "c.json", "--no-alt-screen"]));
    assert_eq!(
        a,
        TuiArgs {
            config: Some("c.json".into()),
            alt_screen: false
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
    // Command derives Debug/Eq for assertions.
    assert!(format!("{:?}", Command::Tui).contains("Tui"));
    assert_ne!(Command::Tui, Command::Daemon);
}

#[test]
fn parses_init_args() {
    // A bare `init` targets the cwd with every flag off.
    let bare = parse_init_args(&argv(&[]));
    assert_eq!(bare, InitArgs::default());

    let with_dir = parse_init_args(&argv(&["packages/api"]));
    assert_eq!(with_dir.dir.as_deref(), Some("packages/api"));
    assert!(!with_dir.force);
    assert!(!with_dir.offline);

    let full = parse_init_args(&argv(&[
        "packages/api",
        "--force",
        "--offline",
        "--config",
        "/tmp/medulla.toml",
    ]));
    assert_eq!(full.dir.as_deref(), Some("packages/api"));
    assert!(full.force);
    assert!(full.offline);
    assert_eq!(full.config.as_deref(), Some("/tmp/medulla.toml"));

    // Short form, and flags before the directory.
    let short = parse_init_args(&argv(&["-f", "docs"]));
    assert!(short.force);
    assert_eq!(short.dir.as_deref(), Some("docs"));

    // The config VALUE must not be mistaken for the directory.
    let cfg_only = parse_init_args(&argv(&["--config", "/tmp/c.toml"]));
    assert_eq!(cfg_only.dir, None);
    assert_eq!(cfg_only.config.as_deref(), Some("/tmp/c.toml"));

    // Only the first bare word is taken as the directory.
    let two = parse_init_args(&argv(&["first", "second"]));
    assert_eq!(two.dir.as_deref(), Some("first"));
}

#[test]
fn help_lists_init() {
    let help = help_text();
    assert!(help.contains("medulla init"));
    assert!(help.contains("--offline"));
}
