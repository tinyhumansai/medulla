//! Unit tests for provider detection, argv building, the run helpers, and the
//! [`Abort`] handle. Moved verbatim from the former inline `#[cfg(test)] mod
//! tests`; the wildcard `use super::*` is replaced with explicit imports because
//! the logic now lives in sibling `detect`/`execute` modules.

use std::collections::HashMap;
use std::time::Duration;

use crate::tinyplace_support::HarnessProvider;

use super::detect::{
    build_run_args, detect_providers, make_path_lookup, provider_bin, provider_name,
};
use super::execute::{
    extract_claude_result, is_transient_lock, non_empty, rand_unit, tail_bytes, with_auth_hint,
    TAIL_CAP,
};
use super::types::{Abort, ExistsOnPath};

#[test]
fn build_run_args_per_provider() {
    assert_eq!(
        build_run_args(HarnessProvider::Claude, "hello", None, None, &[], false),
        vec!["-p", "--output-format", "stream-json", "--verbose", "hello"]
    );
    assert_eq!(
        build_run_args(HarnessProvider::Claude, "hi", None, None, &[], true),
        vec![
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--dangerously-skip-permissions",
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
            "task",
        ]
    );
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
    // extra args precede the (space-neutralized) prompt.
    assert_eq!(args[args.len() - 2], "--mcp");
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
