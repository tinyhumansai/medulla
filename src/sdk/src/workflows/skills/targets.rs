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
        SkillScope::Managed => managed_root(env, cwd),
        SkillScope::User => env
            .get("HOME")
            .map(|home| home.trim())
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.to_path_buf()),
    }
}

/// Medulla's own root for managed skills: the Medulla home, under which each
/// harness gets its own `<harness>-skills` directory.
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
/// in someone's repository. The home is already per-account
/// (`<root>/<user id>`), so two accounts on one machine do not share skills.
pub fn managed_root(env: &HashMap<String, String>) -> PathBuf {
    crate::home::medulla_home(env)
}

/// One harness's managed directory: `<medulla home>/<harness>-skills`.
///
/// Per harness rather than one shared directory because the thing Medulla hands
/// the harness is a *directory path* — `--add-dir` for Claude — and that grant
/// should not also expose another harness's files. The name says which harness
/// it is for, since it appears verbatim in the operator's session transcript.
///
/// Laid out inside like a project root, so Claude finds
/// `<medulla home>/claude-skills/.claude/skills/<slug>/SKILL.md`.
pub fn managed_dir(target: SkillTarget, env: &HashMap<String, String>) -> PathBuf {
    managed_root(env).join(format!("{}-skills", target.as_str()))
}

/// The root one target actually writes under, given the scope.
///
/// Every scope but `Managed` shares a single root: `$HOME`, the checkout, or an
/// explicit `--dir`. `Managed` splits per harness, so this is the one place
/// that difference lives — callers pass the scope's root and get the target's.
pub(crate) fn target_root(target: SkillTarget, scope: SkillScope, root: &Path) -> PathBuf {
    match scope {
        SkillScope::Managed => root.join(format!("{}-skills", target.as_str())),
        SkillScope::User | SkillScope::Project => root.to_path_buf(),
    }
}

/// The harness's own dotted directory under `root` (`.claude`, `.codex`,
/// `.medulla`). Its existence is what [`default_targets`] reads as "this
/// operator uses this harness".
///
/// Note this is *not* where Codex skills go — see [`skills_dir`]. Codex still
/// keeps its config, prompts, and credentials in `.codex`, so that is still the
/// right signal for "this operator uses Codex".
pub(crate) fn base_dir(target: SkillTarget, root: &Path) -> PathBuf {
    match target {
        SkillTarget::Claude => root.join(".claude"),
        SkillTarget::Codex => root.join(".codex"),
        // `.agents`, not `.medulla`: the generic target exists for harnesses we
        // have not verified individually, and the thing they have in common is
        // the Agent Skills convention. A file under `.medulla/skills` was read
        // by nothing unless the operator wired it up by hand.
        SkillTarget::Generic => root.join(".agents"),
    }
}

/// The legacy Codex skills directory, read but deprecated upstream.
///
/// Codex still scans `$CODEX_HOME/skills` at user scope, and its own source
/// calls it "Deprecated ... kept for backward compatibility" (since
/// `rust-v0.95.0`). We wrote here once; [`super::install`] removes what it finds
/// so the same skill is not discovered twice — a duplicate name makes every
/// `$slug` mention a silent no-op, because Codex drops a mention it cannot
/// resolve to exactly one skill.
pub(crate) fn legacy_codex_skills_dir(root: &Path) -> PathBuf {
    base_dir(SkillTarget::Codex, root).join("skills")
}

/// The directory holding one skill directory per installed workflow.
///
/// Codex reads `.agents/skills`, not `.codex/skills`: the `.agents` convention
/// is the cross-tool [Agent Skills](https://agentskills.io) location, it is the
/// one upstream calls current, and — the part that actually bites — it is
/// anchored to the real home directory rather than to `CODEX_HOME`. An operator
/// with `CODEX_HOME` set to a profile directory would never have seen a skill
/// we wrote to `~/.codex/skills`.
pub(crate) fn skills_dir(target: SkillTarget, root: &Path) -> PathBuf {
    match target {
        SkillTarget::Codex => root.join(".agents").join("skills"),
        _ => base_dir(target, root).join("skills"),
    }
}

/// The targets that write somewhere no earlier target in the list already
/// writes.
///
/// Codex and Generic both resolve to `.agents/skills` — deliberately, it is one
/// shared convention — so naming both would otherwise walk the same directory
/// twice: two report lines for one file on install, and the same file listed
/// under two harness names by `list`. The first target named keeps the
/// directory, so an explicit `--harness codex,generic` reports it as Codex.
///
/// Scope-aware because `Managed` gives each harness its own directory, where
/// the two no longer overlap and both should be visited.
pub(crate) fn dedupe_by_skills_dir(
    targets: &[SkillTarget],
    scope: SkillScope,
    root: &Path,
) -> Vec<SkillTarget> {
    let mut seen: Vec<PathBuf> = Vec::with_capacity(targets.len());
    let mut kept = Vec::with_capacity(targets.len());
    for target in targets {
        let dir = skills_dir(*target, &target_root(*target, scope, root));
        if seen.contains(&dir) {
            continue;
        }
        seen.push(dir);
        kept.push(*target);
    }
    kept
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
    let dir = managed_dir(SkillTarget::Claude, env);
    if !skills_dir(SkillTarget::Claude, &dir).is_dir() {
        return Vec::new();
    }
    vec!["--add-dir".to_string(), dir.display().to_string()]
}
