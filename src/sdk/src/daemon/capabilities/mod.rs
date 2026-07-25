//! Capability discovery, ported from the tinyplace CLI `daemon/capabilities.ts`.
//!
//! An orchestrator picking a lane needs the repo, branch, accessible dirs, and
//! the tools/MCP servers an agent can reach. Config heuristics get that wrong, so
//! the daemon asks the agent itself with a short strict-JSON prompt run through
//! the ordinary provider path — then merges the reply over the cheap facts it can
//! establish authoritatively (cwd, git project/branch, detected providers), which
//! win. The probe never fails: a missing/wedged provider degrades to the facts
//! plus empty arrays.
//!
//! The probe prompt is grounded in the workspace's CLAUDE.md/AGENTS.md/README.md
//! (see [`super::dir_context`]) so `summary` carries a ≤100-token project digest;
//! a deterministic digest of those files backs it up when the probe fails.

use std::collections::HashMap;

use tokio::process::Command;

use crate::tinyplace::{AgentCapabilities, HarnessProvider};

use super::dir_context::{read_dir_context, truncate_chars, MAX_SUMMARY_CHARS};
use super::providers::{Abort, RunTaskFn, RunTaskOptions};

/// The strict-JSON self-report prompt.
pub const CAPABILITY_PROMPT: &str = "Report your own capabilities for an orchestrator. Respond with ONLY a JSON object, no prose or markdown, matching {\"tools\":string[],\"mcpServers\":string[],\"accessibleDirs\":string[],\"summary\":string}: tools=tool/command names you can invoke; mcpServers=MCP servers/connectors available to you; accessibleDirs=absolute dirs you can read/write; summary=at most 100 tokens: what this project/directory is (drawn from the project files below when present), its key conventions, and what you can do here.";

/// A capability probe should answer in seconds; a slow one must not stall a query.
pub const DEFAULT_PROBE_TIMEOUT_MS: u64 = 60_000;

/// Ask the agent what it can do, merged over the facts we already know. Never
/// fails — a failed probe yields the cheap facts and empty arrays.
pub async fn probe_capabilities(options: ProbeOptions) -> AgentCapabilities {
    let cwd = resolve_path(&options.workspace);
    let git = read_git_facts(&cwd).await;
    let dir = read_dir_context(&cwd).await;

    // Best-effort budget/readiness for the offered harnesses. Fails open: this
    // never errors and only reports installed providers, so an unusable machine
    // simply advertises fewer (or no) budgets rather than blocking the report.
    let mut seams = BudgetSeams::from_env(&options.env);
    if let Some(budget) = &options.budget {
        seams = seams.with_configured(budget.clone());
    }
    let (readiness, budgets) = probe_budgets(&options.providers, &seams);

    let base = AgentCapabilities {
        cwd: Some(cwd.clone()),
        accessible_dirs: unique(
            std::iter::once(cwd.clone()).chain(
                options
                    .accessible_dirs
                    .iter()
                    .map(|path| resolve_path(path)),
            ),
        ),
        project: git.project.clone(),
        branch: git.branch.clone(),
        providers: options.providers.clone(),
        tools: Vec::new(),
        mcp_servers: Vec::new(),
        // Deterministic digest of CLAUDE.md/AGENTS.md/README.md — the summary
        // of last resort so a failed probe still carries project context.
        summary: dir.fallback_summary.clone(),
        // Budget/readiness are established from the environment, not the agent's
        // self-report, so they survive a failed probe (which returns `base`).
        budgets,
        readiness,
    };

    let prompt = match &dir.prompt_block {
        Some(block) => format!("{CAPABILITY_PROMPT}\n\n{block}"),
        None => CAPABILITY_PROMPT.to_string(),
    };
    let run_options = RunTaskOptions {
        // Unattributed by design: the probe asks this machine about itself, on
        // no peer's behalf, so it must not join any conversation.
        conversation: String::new(),
        resume_session_id: None,
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
        // The probe self-reports about this machine; it is not a routed task.
        router: None,
        on_event: None,
        on_stdin: None,
    };

    let reply = match (options.run_task)(run_options).await {
        Ok(result) => result.reply,
        Err(_) => return base, // missing/wedged provider → facts only.
    };

    let reported = parse_capability_reply(&reply);
    let mut merged = base;
    merged.accessible_dirs = unique(
        std::iter::once(cwd)
            .chain(options.accessible_dirs)
            .chain(reported.accessible_dirs),
    );
    merged.tools = reported.tools;
    merged.mcp_servers = reported.mcp_servers;
    merged.summary = reported.summary.or(dir.fallback_summary);
    merged
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
mod tests;

#[cfg(test)]
mod budget_tests;

pub mod budget;
pub use budget::{
    evaluate_provider, probe_budgets, BudgetSeams, ConfiguredBudget, ProviderProbeInput,
};

mod types;
pub use types::GitFacts;
pub use types::ProbeOptions;
use types::ReportedCapabilities;
