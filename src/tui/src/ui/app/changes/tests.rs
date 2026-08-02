//! Focused parsing and repository-boundary tests for the Changes tab.

use std::fs;
use std::process::Command;

use tempfile::tempdir;

use super::repository;

#[test]
fn name_status_keeps_simple_paths_and_rename_destinations() {
    let files = repository::parse_name_status(b"M\0src/main.rs\0R100\0old.rs\0new.rs\0");
    assert_eq!(files[0].status, "M");
    assert_eq!(files[0].path, std::path::Path::new("src/main.rs"));
    assert_eq!(files[1].status, "R");
    assert_eq!(files[1].path, std::path::Path::new("new.rs"));
}

#[test]
fn load_combines_committed_and_untracked_changes_since_baseline() {
    let directory = tempdir().expect("temp repo");
    git(directory.path(), &["init"]);
    git(
        directory.path(),
        &["config", "user.email", "test@example.com"],
    );
    git(directory.path(), &["config", "user.name", "Test"]);
    fs::write(directory.path().join("tracked.txt"), "one\n").expect("write tracked");
    git(directory.path(), &["add", "tracked.txt"]);
    git(directory.path(), &["commit", "-m", "baseline"]);
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    fs::write(directory.path().join("tracked.txt"), "two\n").expect("edit tracked");
    fs::write(directory.path().join("new.txt"), "new\n").expect("write untracked");
    let (commits, files) = repository::load(directory.path(), baseline.trim()).expect("load");

    assert!(commits.is_empty());
    assert_eq!(
        files
            .iter()
            .map(|file| file.path.as_path())
            .collect::<Vec<_>>(),
        vec![std::path::Path::new("new.txt"), std::path::Path::new("tracked.txt")]
    );
    let patch = repository::patch(
        directory.path(),
        baseline.trim(),
        std::path::Path::new("tracked.txt"),
    )
    .expect("patch");
    assert!(patch.iter().any(|line| line == "+two"));
}

#[test]
fn patch_uses_the_baseline_for_deleted_files() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("deleted.txt"), "gone\n").expect("write tracked");
    git(directory.path(), &["add", "deleted.txt"]);
    git(directory.path(), &["commit", "-m", "baseline"]);
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);
    fs::remove_file(directory.path().join("deleted.txt")).expect("delete tracked");

    let patch = repository::patch(
        directory.path(),
        baseline.trim(),
        std::path::Path::new("deleted.txt"),
    )
    .expect("deleted patch");

    assert!(patch.iter().any(|line| line == "-gone"));
}

#[test]
fn load_preserves_paths_that_git_would_quote() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("café\tfile.txt"), "new\n").expect("write unusual path");
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    let (_, files) = repository::load(directory.path(), baseline.trim()).expect("load");
    assert_eq!(files[0].path, std::path::Path::new("café\tfile.txt"));
    let patch = repository::patch(directory.path(), baseline.trim(), &files[0].path)
        .expect("untracked patch");
    assert!(patch.iter().any(|line| line == "+new"));
}

/// Initialize a repository with one empty baseline commit.
fn init_repo(root: &std::path::Path) {
    git(root, &["init"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["commit", "--allow-empty", "-m", "baseline"]);
}

/// Run a Git command that must succeed.
fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

/// Run Git and return stdout from a command that must succeed.
fn output(root: &std::path::Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(result.status.success(), "git {args:?}");
    String::from_utf8(result.stdout).expect("utf8")
}
