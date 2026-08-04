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
        SkillScope::Managed => managed_root(env),
        SkillScope::User => env
            .get("HOME")
            .map(|home| home.trim())
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.to_path_buf()),
    }
}

/// The directory name, under the Medulla home, holding harness-facing files.
const MANAGED_DIR: &str = "harness";

/// Medulla's own skills root: a directory Medulla owns outright, laid out the
/// way a *project* root is (`<root>/.claude/skills/…`).
///
/// This exists because the alternative ways to give a spawned harness a skill
/// are both wrong. Writing into the operator's `~/.claude` mixes generated
/// files into a directory they curate by hand, and relocating the harness's
/// whole config directory (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) takes their
/// credentials and settings with it — a session started that way is not logged
/// in. A separate root that the harness is *pointed at* leaves both alone.
///
/// Under the Medulla home rather than the workspace so one install serves every
/// workspace on the machine, and so the directory is not an untracked artifact
/// in someone's repository.
pub fn managed_root(env: &HashMap<String, String>) -> PathBuf {
    crate::home::medulla_home(env).join(MANAGED_DIR)
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

/// The argv Medulla adds so a harness it spawns can see the managed skills.
///
/// Claude Code loads `.claude/skills/` from any directory passed with
/// `--add-dir`. That is a documented exception to `--add-dir` being a
/// file-access grant rather than configuration discovery — the
/// `permissions.additionalDirectories` *setting* grants access without loading
/// skills, so the flag is the only spelling that works.
///
/// Empty unless the managed root actually holds skills for that harness:
/// pointing a session at a directory that does not exist is a permission grant
/// bought for nothing, and it would show up in the operator's transcript as an
/// unexplained extra directory.
///
/// Other providers get nothing yet. Codex reads skills from fixed locations
/// with no additional-directory flag of its own, so the equivalent has to be a
/// different mechanism, not this one.
pub fn spawn_args(
    provider: crate::protocol::HarnessProvider,
    env: &HashMap<String, String>,
) -> Vec<String> {
    if provider != crate::protocol::HarnessProvider::Claude {
        return Vec::new();
    }
    let root = managed_root(env);
    if !skills_dir(SkillTarget::Claude, &root).is_dir() {
        return Vec::new();
    }
    vec!["--add-dir".to_string(), root.display().to_string()]
}
