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
    assert_eq!(parse_command(&argv(&["update"])), Command::Update);
    assert_eq!(parse_command(&argv(&["init"])), Command::Init);
    assert_eq!(parse_command(&argv(&["init", "some/dir"])), Command::Init);
    assert_eq!(
        parse_command(&argv(&["update", "--check"])),
        Command::Update
    );
    assert_eq!(parse_command(&argv(&["--config", "x.json"])), Command::Tui);
    assert_eq!(parse_command(&argv(&["run", "do", "it"])), Command::Run);
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
fn parses_tui_flags() {
    assert_eq!(parse_tui_args(&argv(&[])), TuiArgs::default());
    let a = parse_tui_args(&argv(&["--config", "c.json", "--no-alt-screen"]));
    assert_eq!(
        a,
        TuiArgs {
            config: Some("c.json".into()),
            alt_screen: false,
            mock: false,
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
fn workspace_dispatches_on_its_own_verb() {
    assert_eq!(parse_command(&argv(&["workspace"])), Command::Workspace);
    assert_eq!(
        parse_command(&argv(&["workspace", "add", "."])),
        Command::Workspace
    );
    // The plural reads naturally enough that typing it should not launch the TUI.
    assert_eq!(parse_command(&argv(&["workspaces"])), Command::Workspace);
}

#[test]
fn parses_workspace_args() {
    // A bare `workspace` lists, which is the read-only default.
    let bare = parse_workspace_args(&argv(&[]));
    assert_eq!(bare, WorkspaceArgs::default());
    assert_eq!(bare.action, WorkspaceAction::List);

    // `add` with no directory means the cwd, resolved by the runner.
    assert_eq!(
        parse_workspace_args(&argv(&["add"])).action,
        WorkspaceAction::Add(None)
    );

    let add = parse_workspace_args(&argv(&[
        "add",
        "packages/api",
        "--harness",
        "gpu-box",
        "--force",
        "--offline",
        "--config",
        "/tmp/medulla.toml",
    ]));
    assert_eq!(
        add.action,
        WorkspaceAction::Add(Some("packages/api".to_string()))
    );
    assert_eq!(add.harness.as_deref(), Some("gpu-box"));
    assert!(add.force);
    assert!(add.offline);
    assert_eq!(add.config.as_deref(), Some("/tmp/medulla.toml"));

    assert!(parse_workspace_args(&argv(&["list", "--json"])).json);

    // Remove takes a path or a registry id.
    assert_eq!(
        parse_workspace_args(&argv(&["remove", "ws-api-1234abcd"])).action,
        WorkspaceAction::Remove("ws-api-1234abcd".to_string())
    );
    assert_eq!(
        parse_workspace_args(&argv(&["rm", "."])).action,
        WorkspaceAction::Remove(".".to_string())
    );

    // A `remove` with no target must never guess at one — it lists instead.
    assert_eq!(
        parse_workspace_args(&argv(&["remove"])).action,
        WorkspaceAction::List
    );

    // Flag VALUES must not be mistaken for the action or its operand.
    let cfg_only = parse_workspace_args(&argv(&["--config", "/tmp/c.toml"]));
    assert_eq!(cfg_only.action, WorkspaceAction::List);
    assert_eq!(cfg_only.config.as_deref(), Some("/tmp/c.toml"));
    assert_eq!(
        parse_workspace_args(&argv(&["add", "--harness", "laptop"])).action,
        WorkspaceAction::Add(None)
    );
}

#[test]
fn help_lists_init() {
    let help = help_text();
    assert!(help.contains("medulla init"));
    assert!(help.contains("--offline"));
}

#[test]
fn help_lists_the_workspace_registry_command() {
    let help = help_text();
    assert!(help.contains("medulla workspace"));
    // The three actions must be discoverable without reading the source.
    assert!(help.contains("add [dir]|list|remove"));
    assert!(help.contains("--harness"));
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
    let a = parse_run_args(&argv(&["--config", "c.toml", "reconcile", "the", "world"]))
        .expect("a run with an instruction parses");
    assert_eq!(a.config.as_deref(), Some("c.toml"));
    assert_eq!(a.instruction, "reconcile the world");
}

#[test]
fn run_args_require_an_instruction() {
    // Only flags, no instruction text → a usage error rather than an empty run.
    let err = parse_run_args(&argv(&["--config", "c.toml"]))
        .expect_err("a flags-only run is a usage error");
    assert!(err.contains("instruction"), "{err}");
    // A dangling value-flag consumes the following token, so this is empty too.
    assert!(parse_run_args(&argv(&["--config"])).is_err());
}

#[test]
fn help_text_documents_the_run_command() {
    let help = help_text();
    assert!(help.contains("medulla run"));
}

#[test]
fn run_rejects_the_retired_core_socket_flag() {
    // Loudly, not silently: an unrecognized token joins the instruction, so a
    // quiet removal would submit the flag to the agent as prompt text.
    let err = parse_run_args(&argv(&["--core-socket", "/run/serve.sock", "hello"]))
        .expect_err("the retired flag is a usage error");
    assert!(err.contains("--core-socket"), "{err}");
}

#[test]
fn dispatches_the_workflow_subcommand_under_either_spelling() {
    assert_eq!(parse_command(&argv(&["workflow"])), Command::Workflow);
    assert_eq!(parse_command(&argv(&["workflows"])), Command::Workflow);
    assert_eq!(
        parse_command(&argv(&["workflow", "list"])),
        Command::Workflow
    );
}

#[test]
fn parses_every_workflow_verb_with_its_operand() {
    let cases: Vec<(Vec<&str>, WorkflowAction)> = vec![
        (vec![], WorkflowAction::List),
        (vec!["list"], WorkflowAction::List),
        (vec!["get", "sweep"], WorkflowAction::Get("sweep".into())),
        (
            vec!["create", "sweep"],
            WorkflowAction::Create("sweep".into()),
        ),
        (
            vec!["delete", "sweep"],
            WorkflowAction::Delete("sweep".into()),
        ),
        (
            vec!["apply-ops", "sweep"],
            WorkflowAction::ApplyOps("sweep".into()),
        ),
        (
            vec!["preview-ops", "sweep"],
            WorkflowAction::PreviewOps("sweep".into()),
        ),
        (
            vec!["dry-run", "sweep"],
            WorkflowAction::DryRun("sweep".into()),
        ),
        (vec!["run", "sweep"], WorkflowAction::Run("sweep".into())),
        (vec!["resume", "r1"], WorkflowAction::Resume("r1".into())),
        (vec!["cancel", "r1"], WorkflowAction::Cancel("r1".into())),
        (
            vec!["list-runs", "sweep"],
            WorkflowAction::ListRuns("sweep".into()),
        ),
        (vec!["get-run", "r1"], WorkflowAction::GetRun("r1".into())),
    ];

    for (args, expected) in cases {
        assert_eq!(
            parse_workflow_args(&argv(&args)).action,
            expected,
            "for {args:?}"
        );
    }
}

#[test]
fn workflow_defaults_reads_the_harness_and_model_flags() {
    let parsed = parse_workflow_args(&argv(&[
        "defaults",
        "sweep",
        "--harness",
        "codex",
        "--model",
        "gpt-5-codex",
    ]));

    assert_eq!(parsed.action, WorkflowAction::Defaults("sweep".into()));
    assert_eq!(parsed.harness.as_deref(), Some("codex"));
    assert_eq!(parsed.model.as_deref(), Some("gpt-5-codex"));
}

#[test]
fn workflow_defaults_with_no_flags_is_a_read() {
    // Absent is not the same as empty: absent leaves the setting alone (so the
    // verb prints it), empty clears it.
    let parsed = parse_workflow_args(&argv(&["defaults", "sweep"]));

    assert_eq!(parsed.action, WorkflowAction::Defaults("sweep".into()));
    assert_eq!(parsed.harness, None);
    assert_eq!(parsed.model, None);
}

#[test]
fn workflow_defaults_clears_with_an_empty_string() {
    let parsed = parse_workflow_args(&argv(&["defaults", "sweep", "--harness", ""]));

    assert_eq!(parsed.harness.as_deref(), Some(""));
}

#[test]
fn workflow_aliases_reach_the_same_actions() {
    assert_eq!(
        parse_workflow_args(&argv(&["show", "sweep"])).action,
        WorkflowAction::Get("sweep".into())
    );
    assert_eq!(
        parse_workflow_args(&argv(&["edit", "sweep"])).action,
        WorkflowAction::ApplyOps("sweep".into())
    );
    assert_eq!(
        parse_workflow_args(&argv(&["simulate", "sweep"])).action,
        WorkflowAction::DryRun("sweep".into())
    );
    assert_eq!(
        parse_workflow_args(&argv(&["kinds"])).action,
        WorkflowAction::Catalog(None)
    );
}

#[test]
fn validate_and_catalog_take_an_optional_operand() {
    // `validate` with no id reads a document from stdin; `catalog` with no kind
    // returns every contract. Both are useful bare, unlike the other verbs.
    assert_eq!(
        parse_workflow_args(&argv(&["validate"])).action,
        WorkflowAction::Validate(None)
    );
    assert_eq!(
        parse_workflow_args(&argv(&["validate", "sweep"])).action,
        WorkflowAction::Validate(Some("sweep".into()))
    );
    assert_eq!(
        parse_workflow_args(&argv(&["catalog", "agent"])).action,
        WorkflowAction::Catalog(Some("agent".into()))
    );
}

#[test]
fn a_verb_missing_its_required_operand_falls_back_to_listing() {
    // Listing is the harmless read-only answer to a half-typed command, and
    // matches how `workspace remove` already behaves.
    for verb in ["get", "run", "delete", "apply-ops", "resume", "cancel"] {
        assert_eq!(
            parse_workflow_args(&argv(&[verb])).action,
            WorkflowAction::List,
            "for bare {verb}"
        );
    }
}

#[test]
fn approve_and_reject_accumulate_so_one_resume_can_release_several_gates() {
    let parsed = parse_workflow_args(&argv(&[
        "resume",
        "r1",
        "--approve",
        "review",
        "--approve",
        "deploy",
        "--reject",
        "risky",
    ]));

    assert_eq!(parsed.action, WorkflowAction::Resume("r1".into()));
    assert_eq!(parsed.approve, vec!["review", "deploy"]);
    assert_eq!(parsed.reject, vec!["risky"]);
}

#[test]
fn workflow_flags_are_parsed_alongside_the_verb() {
    let parsed = parse_workflow_args(&argv(&[
        "run",
        "sweep",
        "--input",
        "{\"n\":1}",
        "--run-id",
        "r9",
        "--config",
        "/tmp/c.toml",
    ]));

    assert_eq!(parsed.action, WorkflowAction::Run("sweep".into()));
    assert_eq!(parsed.input.as_deref(), Some("{\"n\":1}"));
    assert_eq!(parsed.run_id.as_deref(), Some("r9"));
    assert_eq!(parsed.config.as_deref(), Some("/tmp/c.toml"));

    let missing = parse_workflow_args(&argv(&[
        "note",
        "sweep",
        "--kind",
        "--text",
        "--reason",
        "--supersedes",
        "--run-id",
        "r10",
    ]));
    assert_eq!(missing.kind, None);
    assert_eq!(missing.text, None);
    assert_eq!(missing.reason, None);
    assert!(missing.supersedes.is_empty());
    assert_eq!(missing.run_id.as_deref(), Some("r10"));
}

#[test]
fn help_text_documents_the_workflow_command() {
    let help = help_text();
    assert!(help.contains("medulla workflow"));
    assert!(help.contains("--approve"));
}

#[test]
fn the_mcp_verb_is_parsed_so_an_acp_session_can_launch_the_tool_server() {
    assert_eq!(
        parse_workflow_args(&argv(&["mcp"])).action,
        WorkflowAction::Mcp
    );
}
