//! Focused parsing and repository-boundary tests for the Changes tab.

use std::fs;
use std::process::Command;

use medulla::ui::git_review::{ChangeOrigin, CommentAnchor};
use tempfile::tempdir;

use super::repository;
use super::types::{ChangedFile, GitChangesState};

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
        vec![
            std::path::Path::new("new.txt"),
            std::path::Path::new("tracked.txt")
        ]
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
    fs::write(directory.path().join("café file.txt"), "new\n").expect("write unusual path");
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    let (_, files) = repository::load(directory.path(), baseline.trim()).expect("load");
    assert_eq!(files[0].path, std::path::Path::new("café file.txt"));
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

#[test]
fn origins_separate_committed_staged_unstaged_and_untracked_paths() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    for name in ["committed.txt", "staged.txt", "unstaged.txt"] {
        fs::write(directory.path().join(name), "one\n").expect("seed file");
    }
    git(directory.path(), &["add", "."]);
    git(directory.path(), &["commit", "-m", "seed"]);
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    fs::write(directory.path().join("committed.txt"), "two\n").expect("edit committed");
    git(directory.path(), &["add", "committed.txt"]);
    git(directory.path(), &["commit", "-m", "session commit"]);
    fs::write(directory.path().join("staged.txt"), "two\n").expect("edit staged");
    git(directory.path(), &["add", "staged.txt"]);
    fs::write(directory.path().join("unstaged.txt"), "two\n").expect("edit unstaged");
    fs::write(directory.path().join("untracked.txt"), "new\n").expect("write untracked");

    let origins = repository::origins(directory.path(), baseline.trim()).expect("origins");
    assert_eq!(
        origins.get(std::path::Path::new("committed.txt")),
        Some(&vec![ChangeOrigin::Committed])
    );
    assert_eq!(
        origins.get(std::path::Path::new("staged.txt")),
        Some(&vec![ChangeOrigin::Staged])
    );
    assert_eq!(
        origins.get(std::path::Path::new("unstaged.txt")),
        Some(&vec![ChangeOrigin::Unstaged])
    );
    assert_eq!(
        origins.get(std::path::Path::new("untracked.txt")),
        Some(&vec![ChangeOrigin::Untracked])
    );
}

#[test]
fn a_path_committed_then_edited_again_reports_both_origins() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("both.txt"), "one\n").expect("seed file");
    git(directory.path(), &["add", "."]);
    git(directory.path(), &["commit", "-m", "seed"]);
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    fs::write(directory.path().join("both.txt"), "two\n").expect("edit");
    git(directory.path(), &["add", "both.txt"]);
    git(directory.path(), &["commit", "-m", "session commit"]);
    fs::write(directory.path().join("both.txt"), "three\n").expect("edit again");

    let (_, files) = repository::load(directory.path(), baseline.trim()).expect("load");
    let file = files
        .iter()
        .find(|file| file.path == std::path::Path::new("both.txt"))
        .expect("both.txt");
    assert_eq!(
        file.origins,
        vec![ChangeOrigin::Committed, ChangeOrigin::Unstaged]
    );
    assert_eq!(file.origin_label(), "commit+unstaged");
}

#[test]
fn the_baseline_stays_put_as_head_advances() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);
    let baseline = baseline.trim().to_owned();

    for step in 0..3 {
        fs::write(directory.path().join(format!("step{step}.txt")), "x\n").expect("write");
        git(directory.path(), &["add", "."]);
        git(directory.path(), &["commit", "-m", &format!("step {step}")]);
    }

    let (commits, files) = repository::load(directory.path(), &baseline).expect("load");
    assert_eq!(commits.len(), 3, "every session commit stays in view");
    assert_eq!(files.len(), 3, "and so does every file they touched");
}

#[test]
fn hunk_navigation_and_anchors_follow_the_review_cursor() {
    let mut state = GitChangesState {
        files: vec![ChangedFile {
            status: "M".into(),
            path: std::path::PathBuf::from("src/lib.rs"),
            origins: vec![ChangeOrigin::Unstaged],
        }],
        ..GitChangesState::default()
    };
    state.patch = [
        "diff --git a/src/lib.rs b/src/lib.rs",
        "--- a/src/lib.rs",
        "+++ b/src/lib.rs",
        "@@ -1,2 +1,3 @@",
        " one",
        "+two",
        "@@ -9,2 +10,3 @@",
        " nine",
        "+ten",
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .collect();
    state.hunks = medulla::ui::git_review::hunks(&state.patch);

    assert_eq!(state.cursor_anchor(), CommentAnchor::Line(0));
    assert!(state.jump_hunk(true));
    assert_eq!(state.cursor, 3);
    assert_eq!(state.cursor_anchor(), CommentAnchor::Hunk(0));
    assert!(state.jump_hunk(true));
    assert_eq!(state.cursor_anchor(), CommentAnchor::Hunk(1));
    assert!(!state.jump_hunk(true), "navigation stops at the last hunk");

    state.move_cursor(1);
    assert_eq!(state.cursor_anchor(), CommentAnchor::Line(7));
    assert_eq!(state.hunk_at_cursor(), Some(1));
    state.move_cursor(100);
    assert_eq!(state.cursor, 8, "the cursor clamps to the patch");
    state.move_cursor(-100);
    assert_eq!(state.cursor, 0);
    assert!(!state.jump_hunk(false));
}

#[test]
fn an_empty_patch_can_still_take_a_file_comment() {
    let state = GitChangesState::default();
    assert_eq!(state.cursor_anchor(), CommentAnchor::File);
    assert!(state.hunk_at_cursor().is_none());
    assert_eq!(state.selected_path(), None);
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
fn a_refresh_picks_up_a_file_created_after_the_first_read() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);
    let mut state = GitChangesState {
        root: Some(directory.path().to_path_buf()),
        baseline: Some(baseline.trim().to_owned()),
        ..GitChangesState::default()
    };
    state.refresh();
    assert!(state.files.is_empty());
    assert!(state.status_message().contains("0 file(s)"));

    fs::write(directory.path().join("late.txt"), "later\n").expect("write");
    state.refresh();
    assert_eq!(state.files.len(), 1);
    assert_eq!(state.files[0].origins, vec![ChangeOrigin::Untracked]);
}

#[test]
fn a_state_without_a_repository_refreshes_into_an_empty_view() {
    let mut state = GitChangesState::default();
    state.refresh();
    assert!(state.files.is_empty());
    assert!(state.commits.is_empty());
    state.reload_patch();
    assert!(state.patch.is_empty());
    assert!(state.hunks.is_empty());
}

#[test]
fn a_directory_outside_any_repository_reads_as_a_plain_sentence() {
    let directory = tempdir().expect("temp dir");
    let error = repository::discover_in(directory.path()).expect_err("not a repository");
    assert_eq!(error, "Not a Git repository · nothing to review here");
}

#[test]
fn a_repository_without_a_commit_says_there_is_nothing_to_compare() {
    let directory = tempdir().expect("temp repo");
    git(directory.path(), &["init"]);
    let error = repository::discover_in(directory.path()).expect_err("no commits");
    assert!(error.contains("no commits yet"), "{error}");
}

#[test]
fn discovery_resolves_the_root_and_pins_the_current_head() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    let (root, baseline) = repository::discover_in(directory.path()).expect("discover");
    assert_eq!(
        root.canonicalize().expect("root"),
        directory.path().canonicalize().expect("temp dir")
    );
    assert_eq!(
        baseline,
        output(directory.path(), &["rev-parse", "HEAD"]).trim()
    );
}
