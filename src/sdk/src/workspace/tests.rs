//! Real-Git tests for the workspace ledger wrappers.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::*;

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.name", "Ledger Test"]);
    git(
        dir.path(),
        &["config", "user.email", "ledger@example.invalid"],
    );
    std::fs::write(dir.path().join("tracked.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "tracked.txt"]);
    git(dir.path(), &["commit", "-qm", "initial ledger"]);
    dir
}

#[test]
fn clean_repository_snapshot_has_branch_and_history() {
    let dir = repository();
    let snapshot = inspect_workspace(dir.path()).unwrap();
    assert!(!snapshot.branch.name.is_empty());
    assert!(!snapshot.branch.detached);
    assert_eq!(snapshot.branch.ahead, 0);
    assert_eq!(snapshot.branch.behind, 0);
    assert!(snapshot.files.is_empty());
    assert_eq!(snapshot.commits[0].subject, "initial ledger");
    assert_eq!(snapshot.commits[0].author, "Ledger Test");
}

#[test]
fn status_tracks_staged_unstaged_untracked_and_rename_paths() {
    let dir = repository();
    std::fs::write(dir.path().join("tracked.txt"), "two\n").unwrap();
    std::fs::write(dir.path().join("staged.txt"), "staged\n").unwrap();
    std::fs::write(dir.path().join("untracked.txt"), "new\n").unwrap();
    git(dir.path(), &["add", "staged.txt"]);
    git(dir.path(), &["mv", "tracked.txt", "renamed.txt"]);

    let changes = status_porcelain(dir.path()).unwrap();
    let renamed = changes
        .iter()
        .find(|change| change.path == Path::new("renamed.txt"))
        .unwrap();
    assert_eq!(
        renamed.original_path.as_deref(),
        Some(Path::new("tracked.txt"))
    );
    assert_eq!(renamed.index_status, 'R');
    assert!(changes
        .iter()
        .any(|change| change.path == Path::new("staged.txt") && change.index_status == 'A'));
    assert!(changes
        .iter()
        .any(|change| { change.path == Path::new("untracked.txt") && change.marker() == "??" }));
}

#[test]
fn diff_names_merge_index_and_worktree_and_path_diff_labels_them() {
    let dir = repository();
    std::fs::write(dir.path().join("tracked.txt"), "two\n").unwrap();
    std::fs::write(dir.path().join("staged.txt"), "staged\n").unwrap();
    git(dir.path(), &["add", "staged.txt"]);

    let names = diff_name_only(dir.path()).unwrap();
    assert_eq!(
        names,
        vec![
            Path::new("staged.txt").to_path_buf(),
            Path::new("tracked.txt").to_path_buf()
        ]
    );
    assert!(diff(dir.path(), Path::new("tracked.txt"))
        .unwrap()
        .contains("--- WORKTREE ---"));
    assert!(diff(dir.path(), Path::new("staged.txt"))
        .unwrap()
        .contains("--- STAGED ---"));
}

#[test]
fn detached_head_is_explicit() {
    let dir = repository();
    git(dir.path(), &["checkout", "--detach", "-q", "HEAD"]);
    let branch = current_branch(dir.path()).unwrap();
    assert!(branch.detached);
    assert!(!branch.name.is_empty());
}

#[test]
fn upstream_divergence_is_reported() {
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-q"]);
    let dir = repository();
    git(
        dir.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(dir.path(), &["push", "-qu", "origin", "HEAD"]);
    std::fs::write(dir.path().join("ahead.txt"), "ahead\n").unwrap();
    git(dir.path(), &["add", "ahead.txt"]);
    git(dir.path(), &["commit", "-qm", "ahead"]);
    assert_eq!(current_branch(dir.path()).unwrap().ahead, 1);
}

#[test]
fn non_repository_returns_a_typed_git_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = inspect_workspace(dir.path()).unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::Git {
            operation: "repository discovery",
            ..
        }
    ));
}
