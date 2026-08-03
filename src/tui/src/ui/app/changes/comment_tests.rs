//! Tests for review comment lifecycle: persistence, invalidation, and refreshes.

use std::fs;
use std::process::Command;

use medulla::ui::git_review::{ChangeOrigin, CommentAnchor};
use tempfile::tempdir;

use super::repository;
use super::types::GitChangesState;

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

#[test]
fn review_comments_never_touch_the_working_tree() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("reviewed.txt"), "one\n").expect("write");
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);
    let before = output(directory.path(), &["status", "--porcelain"]);

    let mut state = GitChangesState {
        root: Some(directory.path().to_path_buf()),
        baseline: Some(baseline.trim().to_owned()),
        ..GitChangesState::default()
    };
    state.refresh();
    let path = state.selected_path().expect("a changed file").to_path_buf();
    state
        .comments
        .upsert(&path, CommentAnchor::File, "look here");
    state
        .comments
        .upsert(&path, CommentAnchor::Line(4), "and here");
    state
        .comments
        .upsert(&path, CommentAnchor::Line(4), "edited in place");

    assert_eq!(state.comments.count_for(&path), 2);
    assert_eq!(
        state.comments.body(&path, CommentAnchor::Line(4)),
        Some("edited in place")
    );
    assert_eq!(
        output(directory.path(), &["status", "--porcelain"]),
        before,
        "reviewing must not change what Git reports"
    );
    assert!(state.status_message().contains("2 comment(s)"));
}

#[test]
fn file_level_comments_survive_all_refreshes() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("file.txt"), "one\ntwo\nthree\n").expect("write");
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    let mut state = GitChangesState {
        root: Some(directory.path().to_path_buf()),
        baseline: Some(baseline.trim().to_owned()),
        ..GitChangesState::default()
    };
    state.refresh();

    let path = state.selected_path().expect("a changed file").to_path_buf();
    state
        .comments
        .upsert(&path, CommentAnchor::File, "file comment");

    // Edit the file multiple times
    fs::write(
        directory.path().join("file.txt"),
        "zero\none\ntwo\nthree\nfour\n",
    )
    .expect("write");
    state.refresh();

    // File-level comment always survives
    assert_eq!(
        state.comments.body(&path, CommentAnchor::File),
        Some("file comment")
    );
}

#[test]
fn line_comments_become_outdated_when_out_of_bounds() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("file.txt"), "one\ntwo\nthree\n").expect("write");
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    let mut state = GitChangesState {
        root: Some(directory.path().to_path_buf()),
        baseline: Some(baseline.trim().to_owned()),
        ..GitChangesState::default()
    };
    state.refresh();

    let path = state.selected_path().expect("a changed file").to_path_buf();
    let initial_patch_len = state.patch.len();

    // Add a line comment near the end
    state.comments.upsert(
        &path,
        CommentAnchor::Line(initial_patch_len - 1),
        "end comment",
    );

    // Replace file with shorter content
    fs::write(directory.path().join("file.txt"), "x\n").expect("write");
    state.refresh();

    // Comment is retained (not deleted) but marked outdated
    let comments: Vec<_> = state.comments.for_path(&path).collect();
    assert_eq!(comments.len(), 1);
    assert!(comments[0].outdated, "Comment should be marked outdated");
    assert_eq!(comments[0].body, "end comment", "Comment text is preserved");
}

#[test]
fn hunk_comments_survive_unchanged_refresh() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("file.txt"), "one\ntwo\nthree\n").expect("write");
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    let mut state = GitChangesState {
        root: Some(directory.path().to_path_buf()),
        baseline: Some(baseline.trim().to_owned()),
        ..GitChangesState::default()
    };
    state.refresh();

    let path = state.selected_path().expect("a changed file").to_path_buf();
    let initial_hunk_count = state.hunks.len();

    // Add a hunk comment
    state
        .comments
        .upsert(&path, CommentAnchor::Hunk(0), "hunk comment");

    // Refresh the patch (even without changing the file)
    state.reload_patch();

    // Reloading an untouched patch must not change the hunk count. This is the
    // premise of the test, so assert it rather than guarding on it — a guard would
    // let the case the test exists to prove pass vacuously.
    assert_eq!(
        state.hunks.len(),
        initial_hunk_count,
        "Reloading an unchanged patch should not alter the hunk count"
    );

    let comments: Vec<_> = state.comments.for_path(&path).collect();
    assert_eq!(comments.len(), 1);
    assert!(
        !comments[0].outdated,
        "Hunk comment should remain live when hunk count unchanged"
    );
    assert_eq!(
        comments[0].body, "hunk comment",
        "Comment text is preserved"
    );
}

#[test]
fn cancelled_changes_remain_visible_in_file_list() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("file.txt"), "baseline\n").expect("write baseline");
    git(directory.path(), &["add", "file.txt"]);
    git(directory.path(), &["commit", "-m", "baseline"]);
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    // Commit a change
    fs::write(directory.path().join("file.txt"), "changed\n").expect("write changed");
    git(directory.path(), &["add", "file.txt"]);
    git(directory.path(), &["commit", "-m", "commit change"]);

    // Restore to baseline in working tree, so the aggregate diff shows no change
    fs::write(directory.path().join("file.txt"), "baseline\n").expect("restore baseline");

    // Despite aggregate diff showing no change, load() should include the file
    // because it has ChangeOrigin::Committed (from the earlier commit)
    let (_, _, files) = repository::load(directory.path(), baseline.trim()).expect("load");
    assert!(
        !files.is_empty(),
        "File with cancelled changes should still appear"
    );
    let file = files
        .iter()
        .find(|f| f.path == std::path::Path::new("file.txt"))
        .expect("file.txt");
    assert!(
        file.origins.contains(&ChangeOrigin::Committed),
        "Should report Committed origin even when change is cancelled"
    );
}
