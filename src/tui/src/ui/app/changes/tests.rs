//! Focused parsing and repository-boundary tests for the Changes tab.

use std::fs;
use std::process::Command;

use medulla::ui::git_review::{ChangeOrigin, CommentAnchor};
use tempfile::tempdir;

use super::types::{BaselineSource, ChangedFile, GitChangesState};
use super::{launch_baseline, repository, select_harness_baseline};

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
    let (commits, _, files) = repository::load(directory.path(), baseline.trim()).expect("load");

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
fn a_reverted_tracked_file_does_not_become_an_untracked_patch() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("tracked.txt"), "baseline\n").expect("write tracked");
    git(directory.path(), &["add", "tracked.txt"]);
    git(directory.path(), &["commit", "-m", "tracked baseline"]);
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    let patch = repository::patch(
        directory.path(),
        baseline.trim(),
        std::path::Path::new("tracked.txt"),
    )
    .expect("reverted tracked patch");

    assert!(patch.is_empty());
}

#[test]
fn an_unborn_repository_uses_the_empty_tree_as_its_baseline() {
    let directory = tempdir().expect("temp repo");
    git(directory.path(), &["init"]);

    let baseline = repository::resolve_baseline(directory.path()).expect("empty-tree baseline");
    let expected = output(directory.path(), &["hash-object", "-t", "tree", "--stdin"]);

    assert_eq!(baseline, expected.trim());
}

#[test]
fn load_an_unborn_repository_lists_initial_files_without_commits() {
    let directory = tempdir().expect("temp repo");
    git(directory.path(), &["init"]);
    fs::write(directory.path().join("initial.txt"), "initial\n").expect("write initial file");
    let baseline = repository::resolve_baseline(directory.path()).expect("empty-tree baseline");

    let (commits, _, files) =
        repository::load(directory.path(), &baseline).expect("load unborn repo");

    assert!(commits.is_empty());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, std::path::Path::new("initial.txt"));
}

#[test]
fn an_unborn_harness_derives_an_empty_tree_launch_baseline() {
    let directory = tempdir().expect("temp repo");
    git(directory.path(), &["init"]);
    fs::write(directory.path().join("initial.txt"), "new\n").expect("write initial file");

    let empty_tree = repository::empty_tree(directory.path()).expect("empty tree");
    git(
        directory.path(),
        &["config", "user.email", "test@example.com"],
    );
    git(directory.path(), &["config", "user.name", "Test"]);
    git(directory.path(), &["add", "initial.txt"]);
    git(
        directory.path(),
        &["commit", "-m", "first commit after launch"],
    );

    let baseline = launch_baseline(&directory.path().to_string_lossy(), None)
        .expect("unborn launch baseline after HEAD advances");

    assert_eq!(baseline, empty_tree);
}

#[test]
fn choosing_a_harness_baseline_clears_comments_when_repository_changes() {
    let first = tempdir().expect("first repo");
    let second = tempdir().expect("second repo");
    init_repo(first.path());
    init_repo(second.path());
    let first_root = repository::discover_in(first.path()).expect("first root").0;
    let (second_root, second_baseline) =
        repository::discover_in(second.path()).expect("second root");
    let mut state = GitChangesState {
        root: Some(first_root),
        harness_root: Some(second_root.clone()),
        harness_baseline: Some(second_baseline.clone()),
        ..GitChangesState::default()
    };
    state.comments.upsert(
        std::path::Path::new("src/main.rs"),
        CommentAnchor::File,
        "belongs to the first repository",
    );

    state
        .choose_harness_baseline()
        .expect("switch to harness repository");

    assert_eq!(state.root.as_deref(), Some(second_root.as_path()));
    assert_eq!(state.baseline.as_deref(), Some(second_baseline.as_str()));
    assert_eq!(
        state
            .comments
            .count_for(std::path::Path::new("src/main.rs")),
        0
    );
}

#[test]
fn a_selected_non_git_harness_is_not_replaced_by_another_repository() {
    use crate::worker::pty::{HarnessControl, PtyState, SessionRow};

    let outside = tempdir().expect("non-git directory");
    let repository = tempdir().expect("git repository");
    init_repo(repository.path());
    let head = output(repository.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    let row =
        |id: &str, label: &str, cwd: &std::path::Path, launch_commit, started_at| SessionRow {
            id: id.to_owned(),
            label: label.to_owned(),
            provider: medulla::protocol::HarnessProvider::Codex,
            state: PtyState::Running,
            cwd: cwd.to_string_lossy().into_owned(),
            branch: None,
            launch_commit,
            session_id: None,
            thread_name: None,
            started_at,
            last_output_at: started_at,
            last_error: None,
            busy: false,
            control: HarnessControl::User,
            user_spawned: true,
            attention: None,
        };
    let rows = vec![
        row("selected", "plain", outside.path(), None, 1),
        row("git", "repository", repository.path(), Some(head), 2),
    ];

    let error = select_harness_baseline(rows, Some("selected"))
        .expect_err("selected non-git harness must be reported");

    assert!(
        error.contains("plain is not in a Git repository"),
        "{error}"
    );
}

#[test]
fn a_cached_launch_commit_does_not_bypass_checkout_revalidation() {
    let directory = tempdir().expect("repository directory");
    init_repo(directory.path());
    let head = output(directory.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    fs::remove_dir_all(directory.path().join(".git")).expect("remove checkout metadata");

    assert_eq!(
        launch_baseline(&directory.path().to_string_lossy(), Some(&head)),
        None
    );
}

#[test]
fn a_valid_harness_recovers_after_a_non_git_selection_clears_manual_state() {
    let directory = tempdir().expect("repository");
    init_repo(directory.path());
    let launch = output(directory.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    let mut state = GitChangesState {
        baseline_source: BaselineSource::Manual,
        ..GitChangesState::default()
    };

    state.clear_repository("selected harness is outside Git".to_owned());
    state.follow_harness(directory.path(), &launch);
    state.refresh();

    assert_eq!(state.baseline_source, BaselineSource::HarnessLaunch);
    assert_eq!(state.baseline.as_deref(), Some(launch.as_str()));
    assert_eq!(state.error, None);
}

#[test]
#[cfg(unix)]
fn patch_treats_pathspec_magic_in_a_tracked_filename_literally() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    let path = std::path::Path::new(":(top)magic.txt");
    fs::write(directory.path().join(path), "baseline\n").expect("write tracked file");
    git(directory.path(), &["add", ":(literal):(top)magic.txt"]);
    git(directory.path(), &["commit", "-m", "add magic filename"]);
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);
    fs::write(directory.path().join(path), "changed\n").expect("edit tracked file");

    let patch = repository::patch(directory.path(), baseline.trim(), path).expect("literal patch");

    assert!(patch.iter().any(|line| line == "-baseline"));
    assert!(patch.iter().any(|line| line == "+changed"));
}

#[test]
fn load_preserves_paths_that_git_would_quote() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("café file.txt"), "new\n").expect("write unusual path");
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    let (_, _, files) = repository::load(directory.path(), baseline.trim()).expect("load");
    assert_eq!(files[0].path, std::path::Path::new("café file.txt"));
    let patch = repository::patch(directory.path(), baseline.trim(), &files[0].path)
        .expect("untracked patch");
    assert!(patch.iter().any(|line| line == "+new"));
}

#[test]
fn patch_accepts_an_untracked_path_that_starts_with_a_dash() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("-new.txt"), "new\n").expect("write dash path");
    let baseline = output(directory.path(), &["rev-parse", "HEAD"]);

    let patch = repository::patch(
        directory.path(),
        baseline.trim(),
        std::path::Path::new("-new.txt"),
    )
    .expect("dash-prefixed patch");

    assert!(patch.iter().any(|line| line == "+new"));
}

#[test]
fn git_path_removes_only_the_command_terminator() {
    assert_eq!(
        repository::path_from_line(b"/repo/with space \n".to_vec()),
        std::path::Path::new("/repo/with space ")
    );
    assert_eq!(
        repository::path_from_line(b"C:\\repo\r\n".to_vec()),
        std::path::Path::new("C:\\repo")
    );
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

    let (_, _, files) = repository::load(directory.path(), baseline.trim()).expect("load");
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

    let (commits, _, files) = repository::load(directory.path(), &baseline).expect("load");
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
fn history_commits_are_selectable_by_full_id_and_subject() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    fs::write(directory.path().join("next.txt"), "next\n").expect("write");
    git(directory.path(), &["add", "."]);
    git(directory.path(), &["commit", "-m", "second snapshot"]);
    let head = output(directory.path(), &["rev-parse", "HEAD"]);

    let (_, recent, _) = repository::load(directory.path(), head.trim()).expect("history");

    assert_eq!(recent[0].id, head.trim());
    assert_eq!(recent[0].subject, "second snapshot");
    assert_eq!(
        repository::resolve_commit(directory.path(), &head[..7]).expect("short id"),
        head.trim()
    );
}

#[test]
fn manual_baseline_rejects_unknown_revisions_without_changing_selection() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    let head = output(directory.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    let mut state = GitChangesState {
        root: Some(directory.path().to_owned()),
        baseline: Some(head.clone()),
        ..GitChangesState::default()
    };

    let error = state
        .choose_baseline("definitely-not-a-commit", BaselineSource::Manual)
        .expect_err("invalid revision");

    assert!(error.contains("Unknown commit"));
    assert_eq!(state.baseline.as_deref(), Some(head.as_str()));
    assert_eq!(state.baseline_source, BaselineSource::AppLaunch);
}

#[test]
fn following_a_harness_uses_its_launch_commit_until_operator_selects_another() {
    let directory = tempdir().expect("temp repo");
    init_repo(directory.path());
    let launch = output(directory.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    let mut state = GitChangesState::default();
    let expected_root = repository::discover_in(directory.path())
        .expect("discover repository")
        .0;

    state.follow_harness(directory.path(), &launch);
    state.refresh();

    assert_eq!(state.root.as_deref(), Some(expected_root.as_path()));
    assert_eq!(state.baseline.as_deref(), Some(launch.as_str()));
    assert_eq!(state.harness_baseline.as_deref(), Some(launch.as_str()));
}
