//! Tests for the capabilities module.

use super::*;

#[test]
fn parses_strict_json_reply() {
    let reply = r#"{"tools":["Bash","Read"],"mcpServers":["github"],"accessibleDirs":["/repo","/repo"],"summary":"I can edit code."}"#;
    let reported = parse_capability_reply(reply);
    assert_eq!(reported.tools, vec!["Bash", "Read"]);
    assert_eq!(reported.mcp_servers, vec!["github"]);
    assert_eq!(reported.accessible_dirs, vec!["/repo"]); // deduped
    assert_eq!(reported.summary.as_deref(), Some("I can edit code."));
}

#[test]
fn extracts_json_from_prose_and_fence() {
    let reply = "Sure! Here you go:\n```json\n{\"tools\":[\"Edit\"],\"summary\":\"hi\"}\n```";
    let reported = parse_capability_reply(reply);
    assert_eq!(reported.tools, vec!["Edit"]);
    assert_eq!(reported.summary.as_deref(), Some("hi"));
}

#[test]
fn non_json_reply_becomes_summary() {
    let reported = parse_capability_reply("I can help with Rust code.");
    assert!(reported.tools.is_empty());
    assert!(reported.mcp_servers.is_empty());
    assert_eq!(
        reported.summary.as_deref(),
        Some("I can help with Rust code.")
    );
}

#[test]
fn ignores_braces_inside_strings() {
    let reply = r#"prefix {"summary":"has a } brace","tools":[]} suffix"#;
    let reported = parse_capability_reply(reply);
    assert_eq!(reported.summary.as_deref(), Some("has a } brace"));
}

#[test]
fn handles_escaped_quotes_inside_strings() {
    // A backslash-escaped quote must not end the string, so the following
    // brace stays inside it and the object scanner keeps going.
    let reply = r#"noise {"summary":"a \" and a } brace","tools":[]} tail"#;
    let reported = parse_capability_reply(reply);
    assert_eq!(reported.summary.as_deref(), Some("a \" and a } brace"));
}

#[test]
fn resolve_path_falls_back_for_missing_paths() {
    // A non-existent path can't be canonicalized, so it resolves lexically
    // against the current directory rather than erroring.
    let out = resolve_path("definitely-not-a-real-path-xyz");
    assert!(out.ends_with("definitely-not-a-real-path-xyz"));
}

use std::sync::Arc;

use super::super::providers::{RunTaskFn, RunTaskResult};

fn probe_options(run_task: RunTaskFn) -> ProbeOptions {
    probe_options_in(run_task, ".")
}

fn probe_options_in(run_task: RunTaskFn, workspace: &str) -> ProbeOptions {
    ProbeOptions {
        provider: HarnessProvider::Claude,
        run_task,
        workspace: workspace.to_string(),
        accessible_dirs: Vec::new(),
        env: HashMap::new(),
        providers: vec![HarnessProvider::Claude],
        timeout_ms: Some(1_000),
        model: None,
        agent: None,
        skip_permissions: false,
        abort: Abort::new(),
    }
}

#[tokio::test]
async fn probe_merges_agent_report_over_facts() {
    let run_task: RunTaskFn = Arc::new(|opts| {
        Box::pin(async move {
            Ok(RunTaskResult {
                usage: None,
                session_id: None,
                provider: opts.provider,
                reply: r#"{"tools":["Edit"],"mcpServers":["gh"],"accessibleDirs":["/x"],"summary":"can code"}"#
                    .to_string(),
                events: 0,
            })
        })
    });
    let caps = probe_capabilities(probe_options(run_task)).await;
    assert_eq!(caps.tools, vec!["Edit"]);
    assert_eq!(caps.mcp_servers, vec!["gh"]);
    assert_eq!(caps.summary.as_deref(), Some("can code"));
    // cwd is always the first accessible dir; the reported dir is unioned in.
    assert!(caps.accessible_dirs.iter().any(|d| d == "/x"));
    assert!(caps.cwd.is_some());
    assert_eq!(caps.providers, vec![HarnessProvider::Claude]);
}

#[tokio::test]
async fn probe_degrades_to_facts_when_provider_fails() {
    let run_task: RunTaskFn =
        Arc::new(|_opts| Box::pin(async move { Err("provider wedged".to_string()) }));
    let caps = probe_capabilities(probe_options(run_task)).await;
    assert!(caps.tools.is_empty(), "no tools without a working probe");
    assert!(caps.mcp_servers.is_empty());
    assert!(caps.summary.is_none());
    assert!(caps.cwd.is_some(), "cheap facts survive a failed probe");
}

#[tokio::test]
async fn configured_workspace_allowlist_is_advertised_without_a_provider() {
    let primary = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let run_task: RunTaskFn =
        Arc::new(|_opts| Box::pin(async move { Err("provider wedged".to_string()) }));
    let mut options = probe_options_in(run_task, primary.path().to_str().expect("utf-8 primary"));
    options.accessible_dirs = vec![
        second.path().to_string_lossy().into_owned(),
        primary.path().to_string_lossy().into_owned(),
    ];

    let caps = probe_capabilities(options).await;

    assert_eq!(caps.accessible_dirs.len(), 2);
    assert_eq!(
        caps.accessible_dirs[0],
        primary.path().canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(
        caps.accessible_dirs[1],
        second.path().canonicalize().unwrap().to_string_lossy()
    );
}

#[tokio::test]
async fn probe_prompt_is_grounded_in_workspace_files() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(
        dir.path().join("README.md"),
        "# Widget\n\nA widget library.",
    )
    .await
    .unwrap();
    let seen_prompt: Arc<std::sync::Mutex<String>> = Arc::default();
    let captured = seen_prompt.clone();
    let run_task: RunTaskFn = Arc::new(move |opts| {
        *captured.lock().unwrap() = opts.prompt.clone();
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: r#"{"tools":[],"summary":"widget dev agent"}"#.to_string(),
                events: 0,
            })
        })
    });
    let caps = probe_capabilities(probe_options_in(run_task, dir.path().to_str().unwrap())).await;
    let prompt = seen_prompt.lock().unwrap().clone();
    assert!(prompt.starts_with(CAPABILITY_PROMPT));
    assert!(prompt.contains("--- README.md (excerpt) ---"));
    assert!(prompt.contains("A widget library."));
    // The agent's grounded summary wins over the deterministic digest.
    assert_eq!(caps.summary.as_deref(), Some("widget dev agent"));
}

#[tokio::test]
async fn failed_probe_falls_back_to_dir_digest() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(
        dir.path().join("README.md"),
        "# Widget\n\nA widget library.",
    )
    .await
    .unwrap();
    let run_task: RunTaskFn =
        Arc::new(|_opts| Box::pin(async move { Err("provider wedged".to_string()) }));
    let caps = probe_capabilities(probe_options_in(run_task, dir.path().to_str().unwrap())).await;
    assert_eq!(
        caps.summary.as_deref(),
        Some("README.md: Widget — A widget library.")
    );
}

#[tokio::test]
async fn reply_without_summary_falls_back_to_dir_digest() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("AGENTS.md"), "# Agents\n\nRun cargo test.")
        .await
        .unwrap();
    let run_task: RunTaskFn = Arc::new(|opts| {
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: r#"{"tools":["Edit"]}"#.to_string(),
                events: 0,
            })
        })
    });
    let caps = probe_capabilities(probe_options_in(run_task, dir.path().to_str().unwrap())).await;
    assert_eq!(caps.tools, vec!["Edit"]);
    assert_eq!(
        caps.summary.as_deref(),
        Some("AGENTS.md: Agents — Run cargo test.")
    );
}

#[test]
fn overlong_reported_summary_is_capped() {
    let long = "x".repeat(2_000);
    let reply = format!(r#"{{"summary":"{long}"}}"#);
    let reported = parse_capability_reply(&reply);
    let summary = reported.summary.unwrap();
    assert!(summary.chars().count() <= MAX_SUMMARY_CHARS);
    assert!(summary.ends_with('…'));
}

#[tokio::test]
async fn read_git_facts_on_bogus_path_is_empty() {
    let facts = read_git_facts("/no/such/workspace/anywhere").await;
    assert!(facts.project.is_none());
    assert!(facts.branch.is_none());
}

#[test]
fn repo_name_strips_suffixes_and_tokens() {
    assert_eq!(
        repo_name_from_remote("git@github.com:org/repo.git").as_deref(),
        Some("repo")
    );
    assert_eq!(
        repo_name_from_remote("https://host/org/repo.git").as_deref(),
        Some("repo")
    );
    assert_eq!(
        repo_name_from_remote("https://x-token@host/org/repo?foo=1").as_deref(),
        Some("repo")
    );
    assert_eq!(
        repo_name_from_remote("/path/to/myrepo/").as_deref(),
        Some("myrepo")
    );
}
