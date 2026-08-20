//! What a session says about the Git checkout it is working in: the launch
//! identity that pins its diff baseline, and the repository, worktree, and
//! branch the rail names it by.
//!
//! Kept apart from [`session`](super::session), which is about pty mechanics:
//! these stand up real repositories on disk, and the branch one waits on the
//! poller that keeps the answer current.

use std::process::Command;

use super::*;

#[test]
fn read_only_git_metadata_uses_a_validatable_fallback_identity() {
    let directory = tempfile::tempdir().expect("repository");
    assert!(Command::new("git")
        .current_dir(directory.path())
        .args(["init", "--quiet"])
        .status()
        .expect("git init")
        .success());
    let identity = super::super::checkout::capture_with(directory.path(), |_, _| false)
        .expect("fallback identity");
    let matches = super::super::checkout::matches(directory.path(), &identity);

    assert!(identity.starts_with("metadata:"), "{identity}");
    assert!(matches);
}

/// Run one Git command in `dir`, failing the test if it does not succeed.
fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// Initialize a repository with one commit, so it has a `HEAD` to move.
fn repo_with_a_commit(dir: &std::path::Path) {
    git(dir, &["init", "--quiet", "--initial-branch", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join("file.txt"), "one").unwrap();
    git(dir, &["add", "file.txt"]);
    git(dir, &["commit", "--quiet", "-m", "first"]);
}

#[test]
fn a_branch_created_after_launch_replaces_the_one_it_started_on() {
    // The whole reason the checkout is polled rather than snapshotted: an agent
    // whose first act is to cut a branch would otherwise be filed under the
    // branch the work started from for as long as the session lived.
    let dir = tempfile::tempdir().unwrap();
    repo_with_a_commit(dir.path());

    let manager = PtyManager::new();
    let mut spec = sh("sleep 30");
    spec.cwd = dir.path().to_string_lossy().into_owned();
    let id = manager.open(spec).unwrap();
    assert_eq!(
        manager.row(&id).unwrap().checkout.branch.as_deref(),
        Some("main")
    );

    git(dir.path(), &["switch", "--quiet", "-c", "fix/login"]);

    wait_for("the rail to follow the new branch", || {
        manager.row(&id).unwrap().checkout.branch.as_deref() == Some("fix/login")
    });
    manager.close(&id);
}

#[test]
fn a_session_in_a_linked_worktree_names_it_and_its_repository() {
    let dir = tempfile::tempdir().unwrap();
    let repository = dir.path().join("medulla-public");
    std::fs::create_dir(&repository).unwrap();
    repo_with_a_commit(&repository);
    let linked = dir.path().join("elsewhere");
    git(
        &repository,
        &[
            "worktree",
            "add",
            "-b",
            "fix-login",
            &linked.to_string_lossy(),
        ],
    );

    let manager = PtyManager::new();
    let mut spec = sh("sleep 30");
    spec.cwd = linked.to_string_lossy().into_owned();
    let id = manager.open(spec).unwrap();

    let checkout = manager.row(&id).unwrap().checkout;
    assert!(checkout.is_worktree());
    // The name Git registered the worktree under, which is its directory's —
    // deliberately not the branch, which is a separate field and moves.
    assert_eq!(checkout.worktree.as_deref(), Some("elsewhere"));
    // And filed under the repository every worktree of it shares.
    assert_eq!(checkout.repo.as_deref(), Some("medulla-public"));
    manager.close(&id);
}

#[test]
fn a_detached_head_reports_its_commit_instead_of_a_branch() {
    let dir = tempfile::tempdir().unwrap();
    repo_with_a_commit(dir.path());
    git(dir.path(), &["checkout", "--quiet", "--detach", "HEAD"]);

    let manager = PtyManager::new();
    let mut spec = sh("sleep 30");
    spec.cwd = dir.path().to_string_lossy().into_owned();
    let id = manager.open(spec).unwrap();

    let checkout = manager.row(&id).unwrap().checkout;
    assert_eq!(checkout.branch, None);
    assert!(
        checkout.head.is_some(),
        "a detached HEAD still has a commit"
    );
    assert!(checkout
        .branch_label()
        .is_some_and(|label| label.starts_with('@')));
    manager.close(&id);
}

#[test]
fn a_session_records_its_worktrees_branch() {
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "--quiet", "--initial-branch", "visible-branch"])
        .arg(dir.path())
        .status()
        .unwrap();
    assert!(status.success());

    let manager = PtyManager::new();
    let mut spec = sh("sleep 30");
    spec.cwd = dir.path().to_string_lossy().into_owned();
    let id = manager.open(spec).unwrap();

    assert_eq!(
        manager.row(&id).unwrap().checkout.branch.as_deref(),
        Some("visible-branch")
    );
    manager.close(&id);
}

#[test]
fn a_session_outside_git_has_no_branch() {
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::new();
    let mut spec = sh("sleep 30");
    spec.cwd = dir.path().to_string_lossy().into_owned();
    let id = manager.open(spec).unwrap();

    assert!(manager.row(&id).unwrap().checkout.branch.is_none());
    assert!(manager.row(&id).unwrap().launch_root.is_none());
    manager.close(&id);
}
