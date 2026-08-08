//! Which repository, worktree, and branch a directory belongs to.
//!
//! An operator running four worktrees of one repository sees four sessions
//! whose working directories differ by one path segment and whose branches
//! differ not at all until an agent creates one. This module is what lets a
//! surface tell them apart: it turns raw `git` output into a [`Checkout`] that
//! names the repository, the linked worktree (when it is one), and the branch
//! or detached commit.
//!
//! Deliberately **pure**. Nothing here runs Git — the caller does that and
//! hands the output in, which is what keeps this testable offline and on a
//! machine whose own checkout disagrees with the fixture. The terminal app's
//! `worker::pty::checkout::inspect` is the subprocess half.

mod types;

#[cfg(test)]
mod tests;

pub use types::Checkout;

/// The path component Git puts every linked worktree's metadata under.
const WORKTREES: &str = "worktrees";

/// Build a [`Checkout`] from the Git facts a caller has already read.
///
/// - `git_dir` is `git rev-parse --git-dir`: `.git` in a primary checkout,
///   `<common>/worktrees/<name>` in a linked one. This is the only thing that
///   distinguishes the two, which is why it is asked for at all.
/// - `common_dir` is `git rev-parse --git-common-dir`, shared by every worktree
///   of the repository — so the repository name taken from it is stable across
///   them.
/// - `toplevel` is `git rev-parse --show-toplevel`, used to name the repository
///   when `common_dir` is relative and has no parent to read the name from.
/// - `branch` is `git symbolic-ref --short HEAD`, absent on a detached `HEAD`.
///   Asked with `symbolic-ref` rather than `rev-parse --abbrev-ref` because it
///   also answers on an unborn branch, where `rev-parse` fails outright.
/// - `head` is `git rev-parse --short HEAD`, absent before the first commit.
///
/// An empty `toplevel` (and `git_dir`) means the caller found no repository;
/// the result is then [`Checkout::default`], which reports
/// [`is_repository`](Checkout::is_repository) as false.
pub fn derive(
    git_dir: &str,
    common_dir: &str,
    toplevel: &str,
    branch: Option<&str>,
    head: Option<&str>,
) -> Checkout {
    let toplevel = toplevel.trim();
    if toplevel.is_empty() {
        return Checkout::default();
    }
    Checkout {
        repo: repo_name(common_dir.trim(), toplevel),
        worktree: worktree_name(git_dir.trim()),
        branch: non_empty(branch),
        head: non_empty(head),
    }
}

/// The repository's directory name, from its common Git directory.
///
/// `/w/medulla/.git` and the bare `/w/medulla.git` both name `medulla`. A
/// relative common directory (`.git`, which is what Git prints from the top of
/// a primary checkout) has no parent to read, so the checkout root answers
/// instead — the same name by construction.
fn repo_name(common_dir: &str, toplevel: &str) -> Option<String> {
    let from_common = common_dir
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .find(|component| !component.is_empty() && *component != ".git")
        .map(|component| component.trim_end_matches(".git"))
        .filter(|name| !name.is_empty() && *name != "." && *name != "..");
    from_common
        .or_else(|| last_segment(toplevel))
        .map(str::to_owned)
}

/// The linked worktree's name, or `None` for a primary checkout.
///
/// Keyed off the `worktrees/<name>` component Git itself imposes rather than
/// off comparing the Git and common directories, so it holds whether Git
/// printed those paths relative or absolute.
fn worktree_name(git_dir: &str) -> Option<String> {
    let mut components = git_dir.split(['/', '\\']).filter(|part| !part.is_empty());
    components.find(|part| *part == WORKTREES)?;
    components.next().map(str::to_owned)
}

/// The final component of a path, ignoring trailing separators.
fn last_segment(path: &str) -> Option<&str> {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
}

/// A trimmed value, or `None` when it is absent or blank.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
