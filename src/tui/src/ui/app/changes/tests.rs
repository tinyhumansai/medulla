//! Focused parsing and repository-boundary tests for the Changes tab.

use std::fs;
use std::process::Command;

use tempfile::tempdir;

use super::repository;

#[test]
fn name_status_keeps_simple_paths_and_rename_destinations() {
    let files = repository::parse_name_status("M\tsrc/main.rs\nR100\told.rs\tnew.rs\n");
    assert_eq!(files[0].status, "M");
    assert_eq!(files[0].path, "src/main.rs");
    assert_eq!(files[1].status, "R");
    assert_eq!(files[1].path, "new.rs");
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
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["new.txt", "tracked.txt"]
    );
    let patch = repository::patch(directory.path(), baseline.trim(), "tracked.txt").expect("patch");
    assert!(patch.iter().any(|line| line == "+two"));
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
