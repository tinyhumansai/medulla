//! Capability discovery, ported from the tinyplace CLI `daemon/capabilities.ts`.
//!
//! An orchestrator picking a lane needs the repo, branch, accessible dirs, and
//! the tools/MCP servers an agent can reach. Config heuristics get that wrong, so
//! the daemon asks the agent itself with a short strict-JSON prompt run through
//! the ordinary provider path — then merges the reply over the cheap facts it can
//! establish authoritatively (cwd, git project/branch, detected providers), which
//! win. The probe never fails: a missing/wedged provider degrades to the facts
//! plus empty arrays.

use std::collections::HashMap;

use tokio::process::Command;

use crate::tinyplace_support::{AgentCapabilities, HarnessProvider};

use super::dir_context::{read_dir_context, truncate_chars, MAX_SUMMARY_CHARS};
use super::providers::{Abort, RunTaskFn, RunTaskOptions};

/// The strict-JSON self-report prompt.
pub const CAPABILITY_PROMPT: &str = "Report your own capabilities for an orchestrator. Respond with ONLY a JSON object, no prose or markdown, matching {\"tools\":string[],\"mcpServers\":string[],\"accessibleDirs\":string[],\"summary\":string}: tools=tool/command names you can invoke; mcpServers=MCP servers/connectors available to you; accessibleDirs=absolute dirs you can read/write; summary=at most 100 tokens: what this project/directory is (drawn from the project files below when present), its key conventions, and what you can do here.";

/// A capability probe should answer in seconds; a slow one must not stall a query.
pub const DEFAULT_PROBE_TIMEOUT_MS: u64 = 60_000;

/// Inputs for one capability probe.
pub struct ProbeOptions {
    pub provider: HarnessProvider,
    pub run_task: RunTaskFn,
    pub workspace: String,
    pub env: HashMap<String, String>,
    pub providers: Vec<HarnessProvider>,
    pub timeout_ms: Option<u64>,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub skip_permissions: bool,
    pub abort: Abort,
}

/// Ask the agent what it can do, merged over the facts we already know. Never
/// fails — a failed probe yields the cheap facts and empty arrays.
pub async fn probe_capabilities(options: ProbeOptions) -> AgentCapabilities {
    let cwd = resolve_path(&options.workspace);
    let git = read_git_facts(&cwd).await;
    let dir = read_dir_context(&cwd).await;

    let base = AgentCapabilities {
        cwd: Some(cwd.clone()),
        accessible_dirs: vec![cwd.clone()],
        project: git.project.clone(),
        branch: git.branch.clone(),
        providers: options.providers.clone(),
        tools: Vec::new(),
        mcp_servers: Vec::new(),
        // Deterministic digest of CLAUDE.md/AGENTS.md/README.md — the summary
        // of last resort so a failed probe still carries project context.
        summary: dir.fallback_summary.clone(),
    };

    let prompt = match &dir.prompt_block {
        Some(block) => format!("{CAPABILITY_PROMPT}\n\n{block}"),
        None => CAPABILITY_PROMPT.to_string(),
    };
    let run_options = RunTaskOptions {
        provider: options.provider,
        prompt,
        cwd: cwd.clone(),
        env: options.env.clone(),
        timeout_ms: options.timeout_ms.unwrap_or(DEFAULT_PROBE_TIMEOUT_MS),
        model: options.model.clone(),
        agent: options.agent.clone(),
        extra_args: Vec::new(),
        skip_permissions: options.skip_permissions,
        abort: options.abort.clone(),
        on_event: None,
        on_stdin: None,
    };

    let reply = match (options.run_task)(run_options).await {
        Ok(result) => result.reply,
        Err(_) => return base, // missing/wedged provider → facts only.
    };

    let reported = parse_capability_reply(&reply);
    let mut merged = base;
    merged.accessible_dirs = unique(std::iter::once(cwd).chain(reported.accessible_dirs));
    merged.tools = reported.tools;
    merged.mcp_servers = reported.mcp_servers;
    merged.summary = reported.summary.or(dir.fallback_summary);
    merged
}

struct ReportedCapabilities {
    accessible_dirs: Vec<String>,
    tools: Vec<String>,
    mcp_servers: Vec<String>,
    summary: Option<String>,
}

/// Pull the capability object out of a provider reply. Scans for the first
/// brace-balanced `{...}`; a reply with no usable JSON becomes the summary.
fn parse_capability_reply(reply: &str) -> ReportedCapabilities {
    if let Some(json) = first_json_object(reply) {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json) {
            let summary = parsed
                .get("summary")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| truncate_chars(s, MAX_SUMMARY_CHARS));
            return ReportedCapabilities {
                tools: string_array(parsed.get("tools")),
                mcp_servers: string_array(parsed.get("mcpServers")),
                accessible_dirs: string_array(parsed.get("accessibleDirs")),
                summary,
            };
        }
    }
    let raw = reply.trim();
    ReportedCapabilities {
        tools: Vec::new(),
        mcp_servers: Vec::new(),
        accessible_dirs: Vec::new(),
        summary: (!raw.is_empty()).then(|| truncate_chars(raw, MAX_SUMMARY_CHARS)),
    }
}

/// Scan out the first brace-balanced object, ignoring braces inside strings.
fn first_json_object(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.iter().position(|&c| c == '{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for index in start..chars.len() {
        let ch = chars[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(chars[start..=index].iter().collect());
                }
            }
            _ => {}
        }
    }
    None
}

fn string_array(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(serde_json::Value::Array(items)) = value else {
        return Vec::new();
    };
    unique(items.iter().filter_map(|item| {
        item.as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }))
}

fn unique(values: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            out.push(value);
        }
    }
    out
}

/// Git project + branch, best-effort.
#[derive(Debug, Clone, Default)]
pub struct GitFacts {
    pub project: Option<String>,
    pub branch: Option<String>,
}

/// Project + branch from git, best-effort. Runs `git -C <cwd>` so a workspace
/// that does not exist fails as a non-zero exit, not a spawn error.
pub async fn read_git_facts(cwd: &str) -> GitFacts {
    let origin = run_git(&["-C", cwd, "remote", "get-url", "origin"]).await;
    let branch = run_git(&["-C", cwd, "rev-parse", "--abbrev-ref", "HEAD"]).await;
    GitFacts {
        project: origin.as_deref().and_then(repo_name_from_remote),
        branch,
    }
}

/// `git@host:org/repo.git`, `https://host/org/repo.git`, `/path/to/repo` →
/// `repo`. Any `?query`/`#fragment` is dropped first so a token never pollutes
/// the name.
pub fn repo_name_from_remote(remote: &str) -> Option<String> {
    let mut trimmed = remote.trim().to_string();
    if let Some(pos) = trimmed.find(['?', '#']) {
        trimmed.truncate(pos);
    }
    let trimmed = trimmed.trim_end_matches('/');
    let trimmed = trimmed
        .strip_suffix(".git")
        .or_else(|| trimmed.strip_suffix(".GIT"))
        .unwrap_or(trimmed);
    let last = trimmed.rsplit(['/', ':']).next()?;
    let last = last.trim();
    (!last.is_empty()).then(|| last.to_string())
}

async fn run_git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn resolve_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string())
        })
}

#[cfg(test)]
mod tests {
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
                Ok(RunTaskResult { usage: None,
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
                    usage: None,
                    provider: opts.provider,
                    reply: r#"{"tools":[],"summary":"widget dev agent"}"#.to_string(),
                    events: 0,
                })
            })
        });
        let caps =
            probe_capabilities(probe_options_in(run_task, dir.path().to_str().unwrap())).await;
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
        let caps =
            probe_capabilities(probe_options_in(run_task, dir.path().to_str().unwrap())).await;
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
                    usage: None,
                    provider: opts.provider,
                    reply: r#"{"tools":["Edit"]}"#.to_string(),
                    events: 0,
                })
            })
        });
        let caps =
            probe_capabilities(probe_options_in(run_task, dir.path().to_str().unwrap())).await;
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
}
