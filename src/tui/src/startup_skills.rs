//! Startup retirement of legacy ambient workflow skills.
//!
//! Older releases copied generated skills into the harnesses' user roots.
//! That made a workflow skill visible to every Claude and Codex process,
//! including an agent node already executing that same workflow. Current
//! launches attach tools explicitly and attach managed skills where the harness
//! supports it, so startup now removes only marker-owned legacy files and never
//! creates user-scoped integration.

use std::collections::HashMap;
use std::path::Path;

use medulla::workflows::skills::{self, FileAction, InstallOptions, SkillScope, SkillTarget};

/// What startup should surface after its best-effort reconciliation pass.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupSkillsReport {
    /// A one-line success note, present only when startup changed something.
    pub(crate) notice: Option<String>,
    /// Problems that left a generated skill stale, without blocking boot.
    pub(crate) warnings: Vec<String>,
}

/// Remove generated files that older releases exposed at user scope.
pub(crate) fn reconcile(env: &HashMap<String, String>, cwd: &Path) -> StartupSkillsReport {
    // The documented scratch/dev modes promise an isolated Medulla home. A
    // catalog read from that temporary root must never be copied into the
    // operator's real Claude/Codex configuration merely because HOME remains
    // inherited from their shell.
    if uses_isolated_home(env) {
        return StartupSkillsReport::default();
    }
    reconcile_with(env, cwd)
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
fn reconcile_with(env: &HashMap<String, String>, cwd: &Path) -> StartupSkillsReport {
    let root = skills::scope_root(SkillScope::User, env, cwd);
    let targets: Vec<_> = skills::default_targets(&root)
        .into_iter()
        .filter(|target| matches!(target, SkillTarget::Claude | SkillTarget::Codex))
        .collect();
    if targets.is_empty() {
        return StartupSkillsReport::default();
    }

    let mut report = StartupSkillsReport::default();
    let options = InstallOptions {
        targets: targets.clone(),
        scope: SkillScope::User,
        root: root.clone(),
        with_commands: false,
        dry_run: false,
    };

    match skills::sync(&[], &options, true) {
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
                report.notice = Some("retired ambient workflow skills".to_string());
            }
        }
        Err(error) => report.warnings.push(format!(
            "workflow integration: could not retire ambient skills: {error}"
        )),
    }
    report
}

#[cfg(test)]
pub(crate) fn reconcile_for_test(
    env: &HashMap<String, String>,
    cwd: &Path,
    _exe: &Path,
    _claude_program: &Path,
) -> StartupSkillsReport {
    reconcile_with(env, cwd)
}
