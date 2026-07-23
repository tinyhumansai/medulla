//! GitHub CLI process boundary for read probes and explicit ship actions.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::parse::{parse_prs, parse_unresolved_threads};
use super::{PrSummary, ShipError, ShipState, WorkspaceShipReport};

const THREAD_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{isResolved}}}}}";

/// Synchronous `gh` wrapper intended for `spawn_blocking` callers.
#[derive(Debug, Clone)]
pub struct ShipClient {
    binary: PathBuf,
}

impl Default for ShipClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ShipClient {
    /// Use `MEDULLA_GH_BIN` when set, otherwise resolve `gh` through `PATH`.
    pub fn new() -> Self {
        Self {
            binary: std::env::var_os("MEDULLA_GH_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("gh")),
        }
    }

    /// Inject a deterministic GitHub CLI stand-in for tests or embedding.
    pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    /// Inspect all workspaces independently so one bad checkout stays local.
    pub fn inspect_workspaces(&self, roots: &[PathBuf]) -> Vec<WorkspaceShipReport> {
        roots
            .iter()
            .map(|root| WorkspaceShipReport {
                root: root.clone(),
                state: self.inspect_workspace(root),
            })
            .collect()
    }

    /// Inspect open pull requests in one workspace.
    pub fn inspect_workspace(&self, workspace: &Path) -> ShipState {
        if let Err(error) = self.run(workspace, ["auth", "status"]) {
            return ShipState::GhUnavailable(error.to_string());
        }
        match self.inspect_authenticated(workspace) {
            Ok(rows) => ShipState::Ready(rows),
            Err(error) => ShipState::GhUnavailable(error.to_string()),
        }
    }

    /// Fetch a bounded failed-check log excerpt for a selected PR.
    pub fn failing_log_excerpt(&self, workspace: &Path, number: u64) -> Result<String, ShipError> {
        let number = number.to_string();
        let checks = self.run(
            workspace,
            [
                "pr",
                "checks",
                number.as_str(),
                "--json",
                "bucket,link,name",
            ],
        )?;
        let rows: Vec<serde_json::Value> = serde_json::from_str(&checks)?;
        let Some(link) = rows.iter().find_map(|row| {
            (row.get("bucket")?.as_str()? == "fail")
                .then(|| row.get("link")?.as_str().map(str::to_string))
                .flatten()
        }) else {
            return Ok("No failing check log is available.".into());
        };
        let log = self.run(workspace, ["run", "view", link.as_str(), "--log-failed"])?;
        let lines = log.lines().rev().take(40).collect::<Vec<_>>();
        Ok(lines.into_iter().rev().collect::<Vec<_>>().join("\n"))
    }

    /// Open a selected PR through GitHub CLI's browser action.
    pub fn open_pr(&self, workspace: &Path, number: u64) -> Result<(), ShipError> {
        let number = number.to_string();
        self.run(workspace, ["pr", "view", number.as_str(), "--web"])?;
        Ok(())
    }

    /// Create a PR for the current branch against the canonical upstream repo.
    pub fn create_pr(&self, workspace: &Path) -> Result<String, ShipError> {
        let repo = upstream_slug(workspace)?;
        self.run(workspace, ["pr", "create", "--fill", "--repo", &repo])
    }

    fn inspect_authenticated(&self, workspace: &Path) -> Result<Vec<PrSummary>, ShipError> {
        let repo = self.run(
            workspace,
            [
                "repo",
                "view",
                "--json",
                "nameWithOwner",
                "-q",
                ".nameWithOwner",
            ],
        )?;
        let (owner, name) = repo
            .trim()
            .split_once('/')
            .ok_or(ShipError::MissingUpstream)?;
        let list = self.run(
            workspace,
            [
                "pr",
                "list",
                "--state",
                "open",
                "--json",
                "number,title,headRefName,url,statusCheckRollup",
            ],
        )?;
        let mut rows = parse_prs(&list)?;
        for row in &mut rows {
            let number = row.number.to_string();
            let threads = self.run(
                workspace,
                [
                    "api",
                    "graphql",
                    "-f",
                    &format!("query={THREAD_QUERY}"),
                    "-F",
                    &format!("owner={owner}"),
                    "-F",
                    &format!("name={name}"),
                    "-F",
                    &format!("number={number}"),
                ],
            )?;
            row.unresolved_threads = parse_unresolved_threads(&threads)?;
        }
        Ok(rows)
    }

    fn run<I, S>(&self, workspace: &Path, args: I) -> Result<String, ShipError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new(&self.binary)
            .args(args)
            .current_dir(workspace)
            .output()
            .map_err(|error| ShipError::Unavailable(error.to_string()))?;
        output_text(output)
    }
}

/// Turn a process result into trimmed stdout or a useful stderr failure.
fn output_text(output: Output) -> Result<String, ShipError> {
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(ShipError::Command(if detail.is_empty() {
        format!("exit status {}", output.status)
    } else {
        detail
    }))
}

/// Resolve a canonical GitHub slug from `upstream`, falling back to `origin`.
fn upstream_slug(workspace: &Path) -> Result<String, ShipError> {
    for remote in ["upstream", "origin"] {
        let output = Command::new("git")
            .args(["remote", "get-url", remote])
            .current_dir(workspace)
            .output();
        let Ok(output) = output else {
            continue;
        };
        if output.status.success() {
            let raw = String::from_utf8_lossy(&output.stdout);
            if let Some(slug) = parse_remote_slug(raw.trim()) {
                return Ok(slug);
            }
        }
    }
    Err(ShipError::MissingUpstream)
}

/// Accept SSH and HTTPS GitHub remotes and strip a trailing `.git`.
pub(super) fn parse_remote_slug(remote: &str) -> Option<String> {
    let path = remote
        .strip_prefix("git@github.com:")
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))
        .or_else(|| remote.strip_prefix("https://github.com/"))?;
    let slug = path.trim_end_matches(".git").trim_matches('/');
    (slug.split('/').count() == 2).then(|| slug.to_string())
}
