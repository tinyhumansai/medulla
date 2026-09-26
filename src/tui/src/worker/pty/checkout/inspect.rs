//! Reading a directory's repository, worktree, and branch out of Git.
//!
//! The subprocess half of [`medulla::ui::checkout`], which owns the derivation
//! itself. Kept apart from it so the rules stay testable without a repository
//! on disk, and kept out of the render path entirely: every function here
//! forks, so it belongs on a blocking thread — see
//! [`PtyManager::spawn_checkout_poller`](crate::worker::pty::PtyManager::spawn_checkout_poller),
//! which is what keeps a live session's answer current.
//!
//! Nothing here fails loudly. A directory outside Git, a Git that is not
//! installed, and a repository with no first commit are all ordinary states for
//! a harness to run in, and each simply leaves the corresponding field empty.

use std::path::Path;
use std::process::Command;

use medulla::ui::checkout::{derive, Checkout};

/// Resolve which repository, worktree, and branch `cwd` sits in.
///
/// Returns [`Checkout::default`] — a value reporting
/// [`is_repository`](Checkout::is_repository) as false — for a directory
/// outside any repository, so callers never have to distinguish "not a
/// repository" from "Git could not be run".
pub(crate) fn resolve(cwd: &str) -> Checkout {
    // One `rev-parse` for all three paths: they are always asked together, and
    // this is on a per-session timer where three forks would be three times the
    // cost for the same answer.
    let Some(paths) = git(
        cwd,
        &[
            "rev-parse",
            "--git-dir",
            "--git-common-dir",
            "--show-toplevel",
        ],
    ) else {
        return Checkout::default();
    };
    let mut lines = paths.lines();
    let git_dir = lines.next().unwrap_or_default();
    let common_dir = lines.next().unwrap_or_default();
    let toplevel = lines.next().unwrap_or_default();
    derive(
        git_dir,
        common_dir,
        toplevel,
        // `symbolic-ref` rather than `rev-parse --abbrev-ref`, which fails
        // outright on an unborn branch — a repository whose first commit the
        // harness is about to write is exactly the case a fresh checkout is in.
        git(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]).as_deref(),
        git(cwd, &["rev-parse", "--short", "HEAD"]).as_deref(),
    )
}

/// Resolve the checkout for a path, for callers that already hold one.
pub(crate) fn resolve_path(cwd: &Path) -> Checkout {
    resolve(&cwd.to_string_lossy())
}

/// Run one read-only Git command, or `None` if it fails or says nothing.
fn git(cwd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|text| !text.is_empty())
}
