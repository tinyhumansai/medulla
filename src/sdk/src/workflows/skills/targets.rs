//! Where each harness expects to find a skill.
//!
//! Every path decision lives here so that harness churn — and there will be
//! churn, Codex's skill support is young — is a one-file change. Nothing in
//! this module touches the filesystem except [`default_targets`], which only
//! asks whether a directory exists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{SkillScope, SkillTarget};

/// The root a scope resolves to.
///
/// `env` is passed rather than read from the process so tests — and a caller
/// installing into a scratch home — can decide what `HOME` means. An unset or
/// empty `HOME` falls back to `cwd`: writing into the current directory is
/// wrong-ish, but it is recoverable, and panicking in a path helper is not.
pub fn scope_root(scope: SkillScope, env: &HashMap<String, String>, cwd: &Path) -> PathBuf {
    match scope {
        SkillScope::Project => cwd.to_path_buf(),
        SkillScope::User => env
            .get("HOME")
            .map(|home| home.trim())
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.to_path_buf()),
    }
}

/// The harness's own dotted directory under `root` (`.claude`, `.codex`,
/// `.medulla`). Its existence is what [`default_targets`] reads as "this
/// operator uses this harness".
pub(crate) fn base_dir(target: SkillTarget, root: &Path) -> PathBuf {
    match target {
        SkillTarget::Claude => root.join(".claude"),
        SkillTarget::Codex => root.join(".codex"),
        SkillTarget::Generic => root.join(".medulla"),
    }
}

/// The directory holding one skill directory per installed workflow.
pub(crate) fn skills_dir(target: SkillTarget, root: &Path) -> PathBuf {
    base_dir(target, root).join("skills")
}

/// The directory holding slash commands, for targets that have them.
pub(crate) fn commands_dir(target: SkillTarget, root: &Path) -> Option<PathBuf> {
    match target {
        SkillTarget::Claude => Some(base_dir(target, root).join("commands")),
        SkillTarget::Codex => Some(base_dir(target, root).join("prompts")),
        SkillTarget::Generic => None,
    }
}

/// The `SKILL.md` for `slug` under `root`.
pub fn skill_path(target: SkillTarget, root: &Path, slug: &str) -> PathBuf {
    skills_dir(target, root).join(slug).join("SKILL.md")
}

/// The slash-command file for `slug`, or `None` for a target without commands.
///
/// `Generic` has none deliberately: an unverified harness has no command
/// convention to guess at, and a stray markdown file in `.medulla/` would be
/// read by nothing.
pub fn command_path(target: SkillTarget, root: &Path, slug: &str) -> Option<PathBuf> {
    commands_dir(target, root).map(|dir| dir.join(format!("{slug}.md")))
}

/// The targets worth writing when the operator named none.
///
/// "Worth writing" means the harness's directory already exists under `root`:
/// creating `~/.codex` for someone who does not use Codex litters their home
/// with a file nothing reads. An empty result is a legitimate answer, and the
/// caller should say so rather than silently doing nothing.
pub fn default_targets(root: &Path) -> Vec<SkillTarget> {
    SkillTarget::ALL
        .into_iter()
        .filter(|target| base_dir(*target, root).is_dir())
        .collect()
}
