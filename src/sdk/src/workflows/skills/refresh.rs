//! Keeping the managed skills current with the workflow store, at the moment
//! they are about to be read.
//!
//! [`install`](super::install) is an operator action: `medulla skills install`
//! renders the store as it stands and stops. That is right for the harness
//! directories a person curates (`~/.claude`), and wrong for the one Medulla
//! owns. A workflow authored in the TUI, evolved over MCP, disabled, renamed,
//! or deleted changes what a spawned session should know — and until this
//! module existed, nothing rewrote `<medulla home>/claude-skills` afterwards.
//! The operator saw a stale catalog: skills for workflows that no longer exist,
//! nothing at all for the one they just wrote.
//!
//! So the managed scope is refreshed at the door instead. Every place Medulla
//! spawns a harness calls [`refresh_managed`], which re-renders the store into
//! the managed root and *then* hands back the argv pointing the child at it.
//! Whatever the store says at spawn time is what that session sees, with no
//! command to remember to re-run.
//!
//! Three properties make doing this on every spawn affordable and safe:
//!
//! * **It is a diff, not a rewrite.** [`sync`](super::sync) compares each
//!   rendered file against the marker revision already on disk and writes only
//!   what changed, so the steady state is a handful of reads.
//! * **It prunes.** A deleted or disabled workflow's skill is removed, which is
//!   the half an operator cannot do by re-running `install`.
//! * **It only touches Medulla's own root.** The managed scope is
//!   `<medulla home>/skills/scopes/<workspace>/<harness>-skills`, never the
//!   operator's `~/.claude`, so an automatic write here cannot disturb a file
//!   they wrote themselves.
//!
//! Two spawns can happen at once — a worker and a harness pane come up
//! together, or the operator opens two sessions — so the pass is made safe
//! against itself in two ways. The root is scoped to the workspace, because the
//! catalog is: `FileWorkflowStore::discover` layers `<cwd>/.medulla/workflows`
//! under the user-global directory, so a shared root would hand a session
//! another project's skills and let either project's prune delete the other's.
//! And within one workspace the whole load-write-prune pass runs under a
//! cross-process file lock, so a stale listing cannot prune a skill a newer
//! pass has just written.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::protocol::HarnessProvider;
use crate::workflows::store::FileWorkflowStore;
use crate::workflows::types::WorkflowRecord;

use super::install::sync;
use super::targets::{legacy_managed_root, managed_root, spawn_args};
use super::{InstallOptions, InstallReport, SkillScope, SkillTarget};

/// Bring the managed skills up to date and return the argv that points a
/// freshly spawned `provider` at them.
///
/// Drop-in for [`spawn_args`](super::spawn_args), which it calls last: the only
/// difference is that the directory it advertises is guaranteed to describe the
/// store as it is now. `env` is the *child's* environment, so a session running
/// against a scratch `MEDULLA_HOME` refreshes and reads that home's skills;
/// `cwd` is the session's working directory, which is what makes a
/// project-local workflow visible to a session opened inside that project.
///
/// A failed sync is not fatal and is not reported: the skills are a convenience
/// layered onto a session that is otherwise fine, and refusing to launch a
/// harness because a file under our own state directory could not be written
/// would trade a small loss for a total one. Whatever was already installed
/// stays and is still advertised.
pub fn refresh_managed(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
    cwd: &Path,
) -> Vec<String> {
    if let Some(target) = managed_target(provider) {
        let _ = sync_managed(target, env, cwd);
    }
    spawn_args(provider, env, cwd)
}

/// Re-render the store into `target`'s managed directory, pruning what no
/// longer belongs there.
///
/// Separate from [`refresh_managed`] so a test — and any future caller that
/// wants to *report* what changed rather than silently keep going — can see the
/// [`InstallReport`] and the error.
///
/// Commands are not written: a slash command is a thing an operator types, and
/// nobody types into `<medulla home>/claude-skills`. The skill itself is what
/// the model reads.
///
/// Pruning is conditional on a clean load. A document the store could not
/// parse is *absent* from the listing rather than an error, so pruning a
/// half-read store would delete the skill for a workflow that still exists —
/// an operator who saved a malformed edit would lose an unrelated skill along
/// with it. When the load reports any error the pass installs without pruning,
/// which leaves a stale skill behind and is the cheaper of the two mistakes.
///
/// # Errors
///
/// Propagates filesystem errors from the sync.
pub fn sync_managed(
    target: SkillTarget,
    env: &HashMap<String, String>,
    cwd: &Path,
) -> io::Result<InstallReport> {
    let report = FileWorkflowStore::discover(env, cwd).load();
    let workflows: Vec<_> = report
        .workflows
        .iter()
        .map(WorkflowRecord::summary)
        .collect();
    let opts = InstallOptions {
        targets: vec![target],
        scope: SkillScope::Managed,
        root: managed_root(env),
        with_commands: false,
        dry_run: false,
    };
    sync(&workflows, &opts, report.errors.is_empty())
}

/// The managed directory a spawned `provider` will actually read, if any.
///
/// Only Claude Code today, and for the same reason
/// [`spawn_args`](super::spawn_args) only serves it: pointing a harness at a
/// directory requires the harness to have a flag for that, and Claude's
/// `--add-dir` is the only one. Refreshing a directory no spawned process reads
/// would write files for nobody, so the providers without that flag are left
/// out here too, and the two lists stay in step.
fn managed_target(provider: HarnessProvider) -> Option<SkillTarget> {
    match provider {
        HarnessProvider::Claude => Some(SkillTarget::Claude),
        _ => None,
    }
}
