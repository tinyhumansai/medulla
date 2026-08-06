//! Unit tests for provider detection, argv building, the run helpers, and the
//! [`Abort`] handle. Moved verbatim from the former inline `#[cfg(test)] mod
//! tests`; the wildcard `use super::*` is replaced with explicit imports because
//! the logic now lives in sibling `detect`/`execute` modules.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::protocol::HarnessProvider;

use super::detect::{
    build_run_args, detect_providers, make_path_lookup, provider_bin, provider_name,
};
use super::execute::{
    extract_claude_result, is_transient_lock, non_empty, rand_unit, tail_bytes, with_auth_hint,
    TAIL_CAP,
};
use super::types::{Abort, ExistsOnPath, RunTaskOptions};

#[cfg(unix)]
#[tokio::test]
async fn direct_runs_report_the_session_before_workspace_context() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let harness = dir.path().join("fake-claude");
    std::fs::write(
        &harness,
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"direct-session\",\"cwd\":\"/repo\"}' '{\"type\":\"result\",\"result\":\"done\"}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&harness, std::fs::Permissions::from_mode(0o755)).unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let options = RunTaskOptions {
        conversation: "peer".into(),
        session_class: crate::sessions::SessionClass::Unbound,
        resume_session_id: None,
        workspace_context: Default::default(),
        provider: HarnessProvider::Claude,
        prompt: "go".into(),
        cwd: dir.path().to_string_lossy().into_owned(),
        env: HashMap::from([(
            "TINYPLACE_CLAUDE_BIN".into(),
            harness.to_string_lossy().into_owned(),
        )]),
        timeout_ms: 1_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort: Abort::new(),
        router: None,
        attribution: false,
        hooks: crate::harness_hooks::HooksConfig::default(),
        on_event: None,
        on_stdin: None,
        on_session: Some({
            let order = order.clone();
            Box::new(move |session_id| order.lock().unwrap().push(format!("session:{session_id}")))
        }),
        on_workspace_context: Some({
            let order = order.clone();
            Box::new(move |_| order.lock().unwrap().push("workspace".into()))
        }),
    };

    let result = super::execute::run_provider_task(options).await.unwrap();

    assert_eq!(result.session_id.as_deref(), Some("direct-session"));
    assert_eq!(
        order.lock().unwrap().as_slice(),
        ["session:direct-session", "workspace"]
    );
}

#[test]
fn build_run_args_per_provider() {
    assert_eq!(
        build_run_args(HarnessProvider::Claude, "hello", None, None, &[], false),
        vec![
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--",
            "hello"
        ]
    );
    assert_eq!(
        build_run_args(HarnessProvider::Claude, "hi", None, None, &[], true),
        vec![
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--dangerously-skip-permissions",
            "--",
            "hi"
        ]
    );
    assert_eq!(
        build_run_args(
            HarnessProvider::Codex,
            "do",
            Some("gpt-5"),
            None,
            &[],
            false
        ),
        vec!["exec", "--json", "-m", "gpt-5", "do"]
    );
    // Skip-permissions reaches codex as a sandbox bypass: its default
    // `workspace-write` policy has no network and only the cwd writable, which
    // a worktree task cannot commit or push from.
    assert_eq!(
        build_run_args(HarnessProvider::Codex, "do", None, None, &[], true),
        vec![
            "exec",
            "--json",
            "--dangerously-bypass-approvals-and-sandbox",
            "do"
        ]
    );
    assert_eq!(
        build_run_args(
            HarnessProvider::Opencode,
            "do",
            None,
            Some("plan"),
            &[],
            false
        ),
        vec!["run", "--agent", "plan", "--format", "json", "do"]
    );
}

#[test]
fn build_run_args_neutralizes_dash_prompt() {
    let args = build_run_args(HarnessProvider::Codex, "-rf /", None, None, &[], false);
    assert_eq!(args.last().unwrap(), " -rf /");
}

#[test]
fn provider_bin_env_override_wins() {
    let mut env = HashMap::new();
    env.insert("TINYPLACE_CODEX_BIN".to_string(), "/opt/codex".to_string());
    assert_eq!(provider_bin(HarnessProvider::Codex, &env), "/opt/codex");
    assert_eq!(provider_bin(HarnessProvider::Claude, &env), "claude");
}

#[test]
fn detect_providers_uses_injected_lookup() {
    let env = HashMap::new();
    let lookup: ExistsOnPath = Box::new(|bin: &str| bin == "codex");
    let detected = detect_providers(&env, None, Some(&lookup));
    assert_eq!(detected, vec![HarnessProvider::Codex]);
}

#[test]
fn transient_lock_and_auth_hint() {
    assert!(is_transient_lock("SQLITE_BUSY: database is locked"));
    assert!(is_transient_lock("Error: database is locked"));
    assert!(is_transient_lock("database table is locked"));
    assert!(!is_transient_lock("some other error"));
    assert!(with_auth_hint("unexpected server error").contains("opencode auth login"));
    assert!(with_auth_hint("HTTP 401 Unauthorized").contains("opencode auth login"));
    assert!(with_auth_hint("missing api key").contains("opencode auth login"));
    assert!(with_auth_hint("bad credential").contains("opencode auth login"));
    assert_eq!(with_auth_hint("plain failure"), "plain failure");
}

#[test]
fn build_run_args_opencode_with_model_and_extra() {
    let args = build_run_args(
        HarnessProvider::Opencode,
        "task",
        Some("anthropic/claude"),
        Some("build"),
        &["--foo".to_string()],
        false,
    );
    assert_eq!(
        args,
        vec![
            "run",
            "-m",
            "anthropic/claude",
            "--agent",
            "build",
            "--format",
            "json",
            "--foo",
            "task",
        ]
    );
}

#[test]
fn build_run_args_claude_with_model() {
    // Claude wires the model via the long `--model` flag (not `-m`), after the
    // base flags and before extra args / prompt.
    let args = build_run_args(
        HarnessProvider::Claude,
        "task",
        Some("anthropic/claude-opus-4.8"),
        None,
        &["--mcp".to_string()],
        false,
    );
    assert_eq!(
        args,
        vec![
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "anthropic/claude-opus-4.8",
            "--mcp",
            "--",
            "task",
        ]
    );
}

/// The prompt must survive a *variadic* option sitting last in `extra_args`.
///
/// `--add-dir <directories...>` is the real one: it is how a session is pointed
/// at its managed skills, and without a terminator it reads the prompt as one
/// more directory, leaving claude with no prompt at all.
#[test]
fn build_run_args_claude_terminates_options_before_the_prompt() {
    let args = build_run_args(
        HarnessProvider::Claude,
        "do the thing",
        None,
        None,
        &["--add-dir".to_string(), "/skills".to_string()],
        false,
    );
    let terminator = args
        .iter()
        .position(|arg| arg == "--")
        .expect("the prompt must be separated from option parsing");
    assert_eq!(args.last().unwrap(), "do the thing");
    assert_eq!(terminator, args.len() - 2, "{args:?}");
    assert!(args[..terminator].contains(&"/skills".to_string()));
}

#[test]
fn build_run_args_claude_extra_and_dash_prompt() {
    let args = build_run_args(
        HarnessProvider::Claude,
        "-hi",
        None,
        None,
        &["--mcp".to_string()],
        true,
    );
    // extra args precede the terminator and the (space-neutralized) prompt.
    assert_eq!(args[args.len() - 3], "--mcp");
    assert_eq!(args[args.len() - 2], "--");
    assert_eq!(args.last().unwrap(), " -hi");
    assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
}

#[test]
fn provider_bin_prefers_first_env_key_and_trims() {
    let mut env = HashMap::new();
    // Claude honors TINYVERSE_* before TINYPLACE_*.
    env.insert(
        "TINYVERSE_CLAUDE_BIN".to_string(),
        "  /opt/claude  ".to_string(),
    );
    env.insert(
        "TINYPLACE_CLAUDE_BIN".to_string(),
        "/other/claude".to_string(),
    );
    assert_eq!(provider_bin(HarnessProvider::Claude, &env), "/opt/claude");

    // A whitespace-only override is ignored (falls back to the default).
    let mut blank = HashMap::new();
    blank.insert("TINYPLACE_CODEX_BIN".to_string(), "   ".to_string());
    assert_eq!(provider_bin(HarnessProvider::Codex, &blank), "codex");
}

#[test]
fn provider_names_are_wire_stable() {
    assert_eq!(provider_name(HarnessProvider::Claude), "claude");
    assert_eq!(provider_name(HarnessProvider::Codex), "codex");
    assert_eq!(provider_name(HarnessProvider::Opencode), "opencode");
}

#[test]
fn non_empty_and_tail_bytes_helpers() {
    assert_eq!(non_empty(Some("hi")).as_deref(), Some("hi"));
    assert_eq!(non_empty(Some("")), None);
    assert_eq!(non_empty(None), None);

    let small = "abc";
    assert_eq!(tail_bytes(small), "abc");
    let big = "x".repeat(TAIL_CAP + 100);
    let tail = tail_bytes(&big);
    assert_eq!(tail.len(), TAIL_CAP);
}

#[test]
fn rand_unit_is_in_range() {
    let value = rand_unit();
    assert!((0.0..1.0).contains(&value));
}

#[test]
fn extract_claude_result_reads_result_line() {
    let line = r#"{"type":"result","result":"the answer"}"#;
    assert_eq!(extract_claude_result(line).as_deref(), Some("the answer"));
    // A non-result line yields nothing.
    assert_eq!(extract_claude_result(r#"{"type":"assistant"}"#), None);
    assert_eq!(extract_claude_result("not json"), None);
}

#[test]
fn make_path_lookup_resolves_pathish_and_bare_names() {
    // A path-ish name is probed directly; a missing one is not executable.
    let env = HashMap::new();
    let lookup = make_path_lookup(&env);
    assert!(!lookup("/nonexistent/definitely-not-here"));

    // A bare name is searched across PATH; an empty PATH finds nothing.
    assert!(!lookup("definitely-not-a-real-binary-xyz"));
}

#[cfg(windows)]
#[test]
fn make_path_lookup_resolves_windows_executable_suffixes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("openhuman-core.exe"), b"").unwrap();
    let env = HashMap::from([("PATH".to_string(), dir.path().display().to_string())]);

    assert!(make_path_lookup(&env)("openhuman-core"));
    assert!(make_path_lookup(&env)("openhuman-core.exe"));
}

#[test]
fn router_env_resolution_is_the_spawn_seams_source_of_truth() {
    // The executor folds exactly what the pure resolver emits: the per-provider
    // endpoint var + the key referenced BY NAME (resolved from the daemon env at
    // spawn, never returned here). This pins that contract at the provider seam.
    use crate::config::RouterConfig;
    use crate::protocol::env::router_env;

    let router: RouterConfig = serde_json::from_str(
        r#"{"baseUrl":"https://gw/v1","apiKeyEnv":"MEDULLA_ROUTER_KEY",
            "providers":{"claude":{"baseUrl":"https://gw/anthropic"}}}"#,
    )
    .unwrap();

    // codex → OpenAI-compatible endpoint + key-by-name; provider override absent.
    let codex = router_env(HarnessProvider::Codex, &router);
    assert_eq!(
        codex.env,
        vec![("OPENAI_BASE_URL".to_string(), "https://gw/v1".to_string())]
    );
    assert_eq!(
        codex.secret_env,
        vec![(
            "OPENAI_API_KEY".to_string(),
            "MEDULLA_ROUTER_KEY".to_string()
        )]
    );

    // claude → Anthropic wire, provider-scoped endpoint beats the top-level.
    let claude = router_env(HarnessProvider::Claude, &router);
    assert_eq!(
        claude.env,
        vec![(
            "ANTHROPIC_BASE_URL".to_string(),
            "https://gw/anthropic".to_string()
        )]
    );
    assert_eq!(claude.secret_env[0].0, "ANTHROPIC_AUTH_TOKEN");
    // The resolver never yields the secret value — only its env-var name.
    assert_eq!(claude.secret_env[0].1, "MEDULLA_ROUTER_KEY");
}

#[tokio::test]
async fn abort_cancelled_resolves_when_signalled() {
    let abort = Abort::new();
    assert!(!abort.is_aborted());
    let waiter = abort.clone();
    let handle = tokio::spawn(async move { waiter.cancelled().await });
    abort.abort();
    assert!(abort.is_aborted());
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("cancelled should resolve")
        .unwrap();
    // Already-aborted: cancelled returns immediately.
    abort.cancelled().await;
}
