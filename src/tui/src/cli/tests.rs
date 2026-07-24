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
    assert_eq!(parse_command(&argv(&["commit"])), Command::Commit);
    assert_eq!(
        parse_command(&argv(&["update", "--check"])),
        Command::Update
    );
    assert_eq!(parse_command(&argv(&["--config", "x.json"])), Command::Tui);
    assert_eq!(parse_command(&argv(&["run", "do", "it"])), Command::Run);
}

#[test]
fn commit_args_define_an_exact_boundary() {
    let parsed = parse_commit_args(&argv(&[
        "--workspace",
        "/repo",
        "-m",
        "feat(repo): exact commit",
        "--body",
        "Details",
        "--config",
        "medulla.toml",
        "--allow-shared",
        "--",
        "one.txt",
        "-literal-name",
    ]))
    .unwrap();
    assert_eq!(parsed.workspace, "/repo");
    assert_eq!(parsed.subject, "feat(repo): exact commit");
    assert_eq!(parsed.body.as_deref(), Some("Details"));
    assert_eq!(parsed.config.as_deref(), Some("medulla.toml"));
    assert!(parsed.allow_shared);
    assert_eq!(parsed.paths, vec!["one.txt", "-literal-name"]);
}

#[test]
fn commit_args_require_workspace_subject_and_paths() {
    assert!(parse_commit_args(&argv(&["-m", "feat: x", "x"])).is_err());
    assert!(parse_commit_args(&argv(&["--workspace", ".", "x"])).is_err());
    assert!(parse_commit_args(&argv(&["--workspace", ".", "-m", "feat: x"])).is_err());
    assert!(parse_commit_args(&argv(&["--workspace", ".", "-m", "feat: x", "--bad"])).is_err());
    assert!(parse_commit_args(&argv(&["--workspace"])).is_err());
}

#[test]
fn parsers_ignore_documented_unknown_inputs_and_dangling_optional_values() {
    assert_eq!(
        parse_login_args(&argv(&["--unknown"])).unwrap(),
        LoginArgs::default()
    );
    assert_eq!(parse_memory_args(&argv(&["status", "--k"])).unwrap().k, 5);
    assert_eq!(parse_tui_args(&argv(&["--unknown"])), TuiArgs::default());
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
            alt_screen: false,
            mock: false,
            core_socket: None,
        }
    );
    // A dangling --config keeps the default (None → layered discovery).
    assert_eq!(parse_tui_args(&argv(&["--config"])).config, None);
    // `--core-socket` selects the core runtime at an explicit socket path.
    assert_eq!(
        parse_tui_args(&argv(&["--core-socket", "/run/serve.sock"])).core_socket,
        Some("/run/serve.sock".into())
    );
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

#[test]
fn help_lists_exact_commit() {
    let help = help_text();
    assert!(help.contains("medulla commit"));
    assert!(help.contains("--workspace"));
    assert!(help.contains("--allow-shared"));
}

#[test]
fn tui_args_parse_the_mock_flag() {
    // `--mock` is the only headless route to a working runtime with no backend
    // token: it must skip the login screen entirely.
    let a = parse_tui_args(&argv(&["--mock"]));
    assert!(a.mock);
    assert!(!parse_tui_args(&argv(&["--no-alt-screen"])).mock);
}

#[test]
fn help_text_documents_the_mock_flag() {
    assert!(help_text().contains("--mock"));
}

// The worker screen parses `--workspace` itself, because the daemon's flag
// types are private to the SDK. Pinned here because it decides which directory
// a *remote peer's* harness is allowed to edit.
#[test]
fn worker_flag_values_parse_in_both_spellings() {
    fn flag_value(args: &[String], name: &str) -> Option<String> {
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            if arg == name {
                return it.next().cloned().filter(|v| !v.is_empty());
            }
            if let Some(rest) = arg.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
                return (!rest.is_empty()).then(|| rest.to_string());
            }
        }
        None
    }
    let spaced: Vec<String> = ["--tui", "--workspace", "/repo"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(flag_value(&spaced, "--workspace").as_deref(), Some("/repo"));

    let equals: Vec<String> = ["--tui", "--workspace=/repo"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(flag_value(&equals, "--workspace").as_deref(), Some("/repo"));

    // Absent, or present with nothing after it, both mean "use the default" —
    // never an empty workspace, which would resolve to the filesystem root.
    let bare: Vec<String> = vec!["--tui".to_string()];
    assert_eq!(flag_value(&bare, "--workspace"), None);
    let dangling: Vec<String> = vec!["--workspace".to_string()];
    assert_eq!(flag_value(&dangling, "--workspace"), None);
}

#[test]
fn run_args_join_the_instruction_and_read_flags() {
    let a = parse_run_args(&argv(&[
        "--core-socket",
        "/run/serve.sock",
        "--config",
        "c.toml",
        "reconcile",
        "the",
        "world",
    ]))
    .expect("a run with an instruction parses");
    assert_eq!(a.core_socket.as_deref(), Some("/run/serve.sock"));
    assert_eq!(a.config.as_deref(), Some("c.toml"));
    assert_eq!(a.instruction, "reconcile the world");
}

#[test]
fn run_args_require_an_instruction() {
    // Only flags, no instruction text → a usage error rather than an empty run.
    let err = parse_run_args(&argv(&["--core-socket", "/run/serve.sock"]))
        .expect_err("a flags-only run is a usage error");
    assert!(err.contains("instruction"), "{err}");
    // A dangling value-flag consumes the following token, so this is empty too.
    assert!(parse_run_args(&argv(&["--config"])).is_err());
}

#[test]
fn help_text_documents_the_run_command() {
    let help = help_text();
    assert!(help.contains("medulla run"));
    assert!(help.contains("--core-socket"));
}
