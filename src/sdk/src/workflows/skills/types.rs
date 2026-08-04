//! The data types of skill generation: which harness a file belongs to, where
//! its root is, what a rendered skill contains, and what installing one did.
//!
//! Kept apart from the logic so that a caller wiring a CLI or a TUI action can
//! name every shape it has to render without pulling in the filesystem code.

use std::path::PathBuf;

use serde::Serialize;

/// A harness that reads generated skills from disk.
///
/// The variants are *file layouts*, not products: `Generic` exists so an
/// operator on a harness we have not verified still gets a readable skill under
/// `.medulla/`, rather than nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillTarget {
    /// Claude Code: `.claude/skills/<slug>/SKILL.md`, commands in `.claude/commands/`.
    Claude,
    /// Codex CLI: `.codex/skills/<slug>/SKILL.md`, prompts in `.codex/prompts/`.
    Codex,
    /// The unverified-harness fallback: `.medulla/skills/<slug>/SKILL.md`.
    Generic,
}

impl SkillTarget {
    /// Every target, in the order output should list them.
    pub const ALL: [SkillTarget; 3] = [
        SkillTarget::Claude,
        SkillTarget::Codex,
        SkillTarget::Generic,
    ];

    /// The lowercase wire name, as accepted by `--harness` and emitted in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            SkillTarget::Claude => "claude",
            SkillTarget::Codex => "codex",
            SkillTarget::Generic => "generic",
        }
    }

    /// Parses a `--harness` value.
    ///
    /// Case-insensitive and whitespace-tolerant because the value usually
    /// arrives from a comma-split command line.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the accepted values when `value` is not one.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" => Ok(SkillTarget::Claude),
            "codex" => Ok(SkillTarget::Codex),
            "generic" => Ok(SkillTarget::Generic),
            other => Err(format!(
                "unknown harness `{other}` (expected claude, codex, or generic)"
            )),
        }
    }
}

impl std::fmt::Display for SkillTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which root generated files land in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillScope {
    /// `$HOME` — the skill is available in every session on this machine.
    User,
    /// The project directory — the skill travels with the checkout.
    Project,
    /// Medulla's own root under the Medulla home, which Medulla points the
    /// harnesses it spawns at.
    ///
    /// The operator's own sessions do not read it; sessions Medulla starts do,
    /// because Medulla adds the flag. That is the point: generated files stay
    /// out of a directory the operator curates by hand, and a workflow's agent
    /// steps still get the skills.
    Managed,
}

impl SkillScope {
    /// The lowercase wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            SkillScope::User => "user",
            SkillScope::Project => "project",
            SkillScope::Managed => "managed",
        }
    }

    /// Parses a `--scope` value.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the accepted values when `value` is not one.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "user" => Ok(SkillScope::User),
            "project" => Ok(SkillScope::Project),
            "managed" => Ok(SkillScope::Managed),
            other => Err(format!(
                "unknown scope `{other}` (expected user, project, or managed)"
            )),
        }
    }
}

impl std::fmt::Display for SkillScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One workflow rendered to skill text, before any target decides where it goes.
///
/// [`body`](RenderedSkill::body) is the complete file content, managed marker
/// included, so writing it is a byte copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSkill {
    /// The workflow this skill triggers.
    pub workflow_id: String,
    /// The skill's directory and frontmatter `name` (`medulla-<id>`).
    pub slug: String,
    /// The frontmatter description, including the explicit "use when" clause
    /// that makes the model match on it.
    pub description: String,
    /// The whole `SKILL.md`, marker line first.
    pub body: String,
    /// `sha256` of everything below the marker line: the drift fingerprint.
    pub rev: String,
}

/// Where and how to install, shared by install, sync, uninstall, and listing.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// Which harness layouts to write.
    pub targets: Vec<SkillTarget>,
    /// Which scope `root` stands for. Carried alongside `root` because output
    /// reports it and the caller resolved it from the same pair.
    pub scope: SkillScope,
    /// The already-resolved root — `$HOME` or the project directory. The
    /// filesystem code never re-derives this, so a test can point it anywhere.
    pub root: PathBuf,
    /// Whether to also emit the slash-command variant for targets that have one.
    pub with_commands: bool,
    /// Compute the identical report but write nothing.
    pub dry_run: bool,
}

/// What happened to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileAction {
    /// The file did not exist and was written.
    Created,
    /// A managed file existed with a different `rev` and was rewritten.
    Updated,
    /// A managed file existed with the same `rev`; nothing was written.
    Unchanged,
    /// A file existed without a Medulla marker. Left exactly as it was.
    ///
    /// Strictly "someone else wrote this": a file Medulla generated for a
    /// *different* workflow is [`FileAction::SlugCollision`], because telling
    /// the operator a third party owns a file we wrote sends them looking in
    /// the wrong place.
    SkippedUnmanaged,
    /// Two workflows want the same path, so the loser was not installed.
    ///
    /// Slugs are a lossy view of workflow ids — `deploy.prod` and `deploy-prod`
    /// both slugify to `medulla-deploy-prod` — and the honest response is to
    /// name the clash rather than let one workflow silently shadow the other.
    /// Reported whether the winner was installed in this same run or in an
    /// earlier one.
    SlugCollision,
    /// A managed file was deleted.
    Removed,
}

impl FileAction {
    /// The camelCase wire name, for human-readable output.
    pub fn as_str(self) -> &'static str {
        match self {
            FileAction::Created => "created",
            FileAction::Updated => "updated",
            FileAction::Unchanged => "unchanged",
            FileAction::SkippedUnmanaged => "skippedUnmanaged",
            FileAction::SlugCollision => "slugCollision",
            FileAction::Removed => "removed",
        }
    }
}

impl std::fmt::Display for FileAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One file's outcome, enough for a caller to print a line or fail a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOutcome {
    /// The absolute path considered.
    pub path: PathBuf,
    /// The harness layout it belongs to.
    pub target: SkillTarget,
    /// The workflow it carries.
    pub workflow_id: String,
    /// What was done — or, for [`FileAction::SkippedUnmanaged`] and
    /// [`FileAction::SlugCollision`], not done.
    pub action: FileAction,
}

/// The result of an install, sync, or uninstall: one entry per file considered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    /// Every file the operation looked at, in target-then-workflow order.
    pub files: Vec<FileOutcome>,
}

impl InstallReport {
    /// Whether any file was left alone because its path was already spoken for.
    ///
    /// True for both kinds of clash — a foreign file
    /// ([`FileAction::SkippedUnmanaged`]) and another workflow's generated one
    /// ([`FileAction::SlugCollision`]). The one condition a caller should
    /// surface loudly: either way, a workflow the operator asked to install is
    /// *not* installed.
    pub fn has_collisions(&self) -> bool {
        self.files.iter().any(|file| {
            matches!(
                file.action,
                FileAction::SkippedUnmanaged | FileAction::SlugCollision
            )
        })
    }

    /// How many files carry the given action.
    pub fn count(&self, action: FileAction) -> usize {
        self.files
            .iter()
            .filter(|file| file.action == action)
            .count()
    }
}

/// A managed skill found on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkill {
    /// The workflow named by the file's marker.
    pub workflow_id: String,
    /// The directory name the skill lives under.
    pub slug: String,
    /// Which harness layout it was found in.
    pub target: SkillTarget,
    /// The `SKILL.md` itself.
    pub path: PathBuf,
    /// The `rev` recorded in its marker, for drift comparison.
    pub rev: String,
}
