//! Startup reconciliation for user-facing workflow skills and their MCP server.
//!
//! Medulla-spawned harnesses receive a scoped MCP server on their launch
//! request. Ordinary Claude Code and Codex sessions are different: they read
//! skills and MCP registrations from their user configuration. This module
//! keeps those two halves together whenever the interactive Medulla app starts,
//! so a generated skill never points at a tool the next harness session cannot
//! reach.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};

use medulla::workflows::skills::{
    self, FileAction, InstallOptions, RegistrationOptions, SkillScope, SkillTarget,
};
use medulla::workflows::{FileWorkflowStore, WorkflowRecord};

/// Long enough for a cold CLI start, bounded so integration never wedges boot.
const CLAUDE_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(10);

/// What startup should surface after its best-effort reconciliation pass.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupSkillsReport {
    /// A one-line success note, present only when startup changed something.
    pub(crate) notice: Option<String>,
    /// Problems that left a skill or registration stale, without blocking boot.
    pub(crate) warnings: Vec<String>,
}

/// Synchronize user skills and ensure the detected harnesses can launch MCP.
///
/// This is deliberately best effort: losing generated convenience files must
/// not prevent the operator from opening Medulla. Claude's user registry is an
/// opaque CLI-owned store, so it is updated through `claude mcp`; Codex's
/// documented TOML is merged through the SDK's preserving registration path.
pub(crate) fn reconcile(env: &HashMap<String, String>, cwd: &Path) -> StartupSkillsReport {
    // The documented scratch/dev modes promise an isolated Medulla home. A
    // catalog read from that temporary root must never be copied into the
    // operator's real Claude/Codex configuration merely because HOME remains
    // inherited from their shell.
    if uses_isolated_home(env) {
        return StartupSkillsReport::default();
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            return StartupSkillsReport {
                notice: None,
                warnings: vec![format!(
                    "workflow integration: cannot resolve the Medulla executable: {error}"
                )],
            };
        }
    };
    reconcile_with(env, cwd, &exe, Path::new("claude"), true)
}

/// Whether this process deliberately redirected Medulla away from production state.
fn uses_isolated_home(env: &HashMap<String, String>) -> bool {
    env.get("MEDULLA_HOME")
        .is_some_and(|value| !value.trim().is_empty())
        || env.get("MEDULLA_DEV").is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

/// Injectable implementation used by tests to stand in for the Claude CLI.
fn reconcile_with(
    env: &HashMap<String, String>,
    cwd: &Path,
    exe: &Path,
    claude_program: &Path,
    run_preflight: bool,
) -> StartupSkillsReport {
    let root = skills::scope_root(SkillScope::User, env, cwd);
    let targets: Vec<_> = skills::default_targets(&root)
        .into_iter()
        .filter(|target| matches!(target, SkillTarget::Claude | SkillTarget::Codex))
        .collect();
    if targets.is_empty() {
        return StartupSkillsReport::default();
    }

    let mut report = StartupSkillsReport::default();
    let loaded = FileWorkflowStore::discover(env, cwd).load();
    let workflows: Vec<_> = loaded
        .workflows
        .iter()
        .map(WorkflowRecord::summary)
        .collect();
    let options = InstallOptions {
        targets: targets.clone(),
        scope: SkillScope::User,
        root: root.clone(),
        with_commands: false,
        dry_run: false,
    };

    match skills::sync(&workflows, &options, loaded.errors.is_empty()) {
        Ok(files) => {
            if files.has_collisions() {
                report.warnings.push(
                    "workflow integration: one or more skill paths are owned by another file"
                        .to_string(),
                );
            }
            if files.files.iter().any(|file| {
                matches!(
                    file.action,
                    FileAction::Created | FileAction::Updated | FileAction::Removed
                )
            }) {
                report.notice = Some("workflow skills synchronized".to_string());
            }
        }
        Err(error) => report.warnings.push(format!(
            "workflow integration: could not synchronize skills: {error}"
        )),
    }
    for error in loaded.errors {
        report
            .warnings
            .push(format!("workflow integration: {error}"));
    }

    if run_preflight {
        if let Err(error) = medulla::mcp::preflight(env, cwd) {
            report
                .warnings
                .push(format!("workflow integration: MCP unavailable: {error}"));
            return report;
        }
    }

    let mut registered = false;
    if targets.contains(&SkillTarget::Codex) {
        let options = RegistrationOptions {
            targets: vec![SkillTarget::Codex],
            scope: SkillScope::User,
            root,
            project_dir: cwd.to_path_buf(),
            exe: exe.to_path_buf(),
            tools_mode: "run".to_string(),
            dry_run: false,
        };
        match skills::register(&options) {
            Ok(outcomes) => {
                registered |= outcomes.iter().any(|outcome| outcome.action != "unchanged");
            }
            Err(error) => report.warnings.push(format!(
                "workflow integration: could not register MCP with Codex: {error}"
            )),
        }
    }

    if targets.contains(&SkillTarget::Claude) {
        match ensure_claude_registration(claude_program, exe) {
            Ok(changed) => registered |= changed,
            Err(error) => report.warnings.push(error),
        }
    }

    if registered {
        report.notice = Some(match report.notice {
            Some(note) => format!("{note}; MCP registered for new harness sessions"),
            None => "MCP registered for new harness sessions".to_string(),
        });
    }
    report
}

/// Add Medulla to Claude's CLI-owned user registry when it is absent.
fn ensure_claude_registration(claude: &Path, medulla: &Path) -> Result<bool, String> {
    let mut child = Command::new(claude)
        .args([
            "mcp",
            "add",
            "--scope",
            "user",
            "medulla",
            "--env",
            "MEDULLA_WORKFLOW_TOOLS=run",
            "--",
        ])
        .arg(medulla)
        .arg("mcp")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!("workflow integration: could not run Claude's MCP registrar: {error}")
        })?;
    let deadline = Instant::now() + CLAUDE_REGISTRATION_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "workflow integration: Claude's MCP registrar timed out after 10 seconds"
                        .to_string(),
                );
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "workflow integration: could not wait for Claude's MCP registrar: {error}"
                ));
            }
        }
    };
    if status.success() {
        return Ok(true);
    }

    let mut detail = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut detail);
    }
    let detail = detail.trim();
    // Claude owns this registry and gives `mcp add` no update/idempotent flag.
    // Its explicit already-present refusal is nevertheless the state we want,
    // and avoids `mcp get`, which health-checks by launching the configured
    // server and could make Medulla startup wait on an unrelated broken entry.
    if detail.contains("MCP server medulla already exists") {
        return Ok(false);
    }
    Err(if detail.is_empty() {
        "workflow integration: Claude rejected the Medulla MCP registration".to_string()
    } else {
        format!("workflow integration: Claude rejected the Medulla MCP registration: {detail}")
    })
}

#[cfg(test)]
pub(crate) fn reconcile_for_test(
    env: &HashMap<String, String>,
    cwd: &Path,
    exe: &Path,
    claude_program: &Path,
) -> StartupSkillsReport {
    // The Rust test harness is not a `medulla mcp` entry point, so production's
    // subprocess preflight is covered by the MCP suite rather than repeated here.
    reconcile_with(env, cwd, exe, claude_program, false)
}
