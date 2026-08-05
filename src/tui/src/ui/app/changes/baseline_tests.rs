//! Harness launch-baseline selection and repository-identity tests.

use std::{fs, path::Path, process::Command};

use medulla::ui::git_review::CommentAnchor;
use tempfile::tempdir;

use super::baseline::{launch_baseline, select_harness_baseline};
use super::repository;
use super::types::{BaselineSource, GitChangesState};

#[test]
fn an_unborn_harness_derives_an_empty_tree_launch_baseline() {
    let directory = tempdir().expect("temp repo");
    git(directory.path(), &["init"]);
    fs::write(directory.path().join("initial.txt"), "new\n").expect("write initial file");
    let empty_tree = repository::empty_tree(directory.path()).expect("empty tree");
    configure_repo(directory.path());
    git(directory.path(), &["add", "initial.txt"]);
    git(
        directory.path(),
        &["commit", "-m", "first commit after launch"],
    );
    let root = repository::discover_in(directory.path()).expect("root").0;

    let baseline = launch_baseline(
        &directory.path().to_string_lossy(),
        Some(&root.to_string_lossy()),
        None,
        crate::worker::pty::checkout::capture(directory.path()).as_deref(),
    )
    .expect("unborn launch baseline");

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
        harness_checkout_identity: crate::worker::pty::checkout::capture(second.path()),
        ..GitChangesState::default()
    };
    state.comments.upsert(
        Path::new("src/main.rs"),
        CommentAnchor::File,
        "first repository",
    );

    state.choose_session_baseline().expect("switch repository");

    assert_eq!(state.root.as_deref(), Some(second_root.as_path()));
    assert_eq!(state.baseline.as_deref(), Some(second_baseline.as_str()));
    assert_eq!(state.comments.count_for(Path::new("src/main.rs")), 0);
}

#[test]
fn following_a_new_launch_commit_in_the_same_repository_preserves_comments() {
    let directory = tempdir().expect("repository");
    init_repo(directory.path());
    let first = output(directory.path(), &["rev-parse", "HEAD"]);
    let mut state = GitChangesState::default();
    let identity = crate::worker::pty::checkout::capture(directory.path()).expect("identity");
    state.follow_session(directory.path(), &first, &identity);
    state
        .comments
        .upsert(Path::new("src/main.rs"), CommentAnchor::File, "keep this");
    git(
        directory.path(),
        &["commit", "--allow-empty", "-m", "later launch"],
    );
    let second = output(directory.path(), &["rev-parse", "HEAD"]);

    state.follow_session(directory.path(), &second, &identity);

    assert_eq!(state.baseline.as_deref(), Some(second.as_str()));
    assert_eq!(state.comments.count_for(Path::new("src/main.rs")), 1);
}

#[test]
fn choosing_harness_launch_replaces_an_operator_selected_baseline() {
    let directory = tempdir().expect("repository");
    init_repo(directory.path());
    let launch = output(directory.path(), &["rev-parse", "HEAD"]);
    git(
        directory.path(),
        &["commit", "--allow-empty", "-m", "manual baseline"],
    );
    let manual = output(directory.path(), &["rev-parse", "HEAD"]);
    let identity = crate::worker::pty::checkout::capture(directory.path()).expect("identity");
    let mut state = GitChangesState::default();
    state.follow_session(directory.path(), &launch, &identity);
    state
        .choose_baseline(&manual, BaselineSource::Manual)
        .expect("manual baseline");

    // Opening a selected harness with `d` uses this explicit activation after
    // following it; an ordinary refresh deliberately preserves Manual mode.
    state.follow_session(directory.path(), &launch, &identity);
    state.choose_session_baseline().expect("harness baseline");

    assert_eq!(state.baseline.as_deref(), Some(launch.as_str()));
    assert_eq!(state.baseline_source, BaselineSource::SessionLaunch);
}

#[test]
fn returning_from_a_non_git_harness_preserves_same_repository_comments() {
    let directory = tempdir().expect("repository");
    init_repo(directory.path());
    let launch = output(directory.path(), &["rev-parse", "HEAD"]);
    let identity = crate::worker::pty::checkout::capture(directory.path()).expect("identity");
    let mut state = GitChangesState::default();
    state.follow_session(directory.path(), &launch, &identity);
    state
        .comments
        .upsert(Path::new("src/main.rs"), CommentAnchor::File, "keep this");

    state.clear_repository("selected session is outside Git".to_owned());
    state.follow_session(directory.path(), &launch, &identity);

    assert_eq!(state.comments.count_for(Path::new("src/main.rs")), 1);
}

#[test]
fn applying_session_launch_revalidates_the_checkout_marker() {
    let directory = tempdir().expect("repository");
    init_repo(directory.path());
    let launch = output(directory.path(), &["rev-parse", "HEAD"]);
    let identity = crate::worker::pty::checkout::capture(directory.path()).expect("identity");
    let mut state = GitChangesState::default();
    state.follow_session(directory.path(), &launch, &identity);
    fs::remove_dir_all(directory.path().join(".git")).expect("remove checkout metadata");
    git(directory.path(), &["init"]);

    let error = state
        .choose_session_baseline()
        .expect_err("replacement checkout must be rejected");

    assert_eq!(error, "Session Git checkout changed since launch");
}

#[test]
fn a_selected_non_git_harness_is_not_replaced_by_another_repository() {
    let outside = tempdir().expect("non-git directory");
    let repository_dir = tempdir().expect("git repository");
    init_repo(repository_dir.path());
    let head = output(repository_dir.path(), &["rev-parse", "HEAD"]);
    let rows = vec![
        row("selected", "plain", outside.path(), None, 1),
        row("git", "repository", repository_dir.path(), Some(head), 2),
    ];

    let error = select_harness_baseline(rows, Some("selected")).expect_err("report selection");

    assert!(
        error.contains("plain is not in a Git repository"),
        "{error}"
    );
}

#[test]
fn newest_eligible_harness_is_selected() {
    let older = tempdir().expect("older repository");
    let newer = tempdir().expect("newer repository");
    init_repo(older.path());
    init_repo(newer.path());
    let rows = vec![
        row(
            "older",
            "older",
            older.path(),
            Some(output(older.path(), &["rev-parse", "HEAD"])),
            1,
        ),
        row(
            "newer",
            "newer",
            newer.path(),
            Some(output(newer.path(), &["rev-parse", "HEAD"])),
            2,
        ),
    ];

    let (selected, _) = select_harness_baseline(rows, None)
        .expect("selection")
        .expect("row");

    assert_eq!(selected.id, "newer");
}

#[test]
fn a_cached_launch_commit_does_not_bypass_checkout_revalidation() {
    let directory = tempdir().expect("repository directory");
    init_repo(directory.path());
    let head = output(directory.path(), &["rev-parse", "HEAD"]);
    let root = repository::discover_in(directory.path()).expect("root").0;
    fs::remove_dir_all(directory.path().join(".git")).expect("remove checkout metadata");

    assert_eq!(
        launch_baseline(
            &directory.path().to_string_lossy(),
            Some(&root.to_string_lossy()),
            Some(&head),
            None,
        ),
        None
    );
}

#[test]
fn a_replacement_repository_cannot_reuse_the_cached_launch_commit() {
    let parent = tempdir().expect("repository parent");
    let source = parent.path().join("source");
    let checkout = parent.path().join("checkout");
    fs::create_dir(&source).expect("source directory");
    init_repo(&source);
    git(parent.path(), &["clone", "--quiet", "source", "checkout"]);
    let head = output(&checkout, &["rev-parse", "HEAD"]);
    let root = repository::discover_in(&checkout).expect("root").0;
    let launch_identity = crate::worker::pty::checkout::capture(&checkout);
    fs::remove_dir_all(&checkout).expect("remove original checkout");
    git(parent.path(), &["clone", "--quiet", "source", "checkout"]);

    assert_eq!(
        launch_baseline(
            &checkout.to_string_lossy(),
            Some(&root.to_string_lossy()),
            Some(&head),
            launch_identity.as_deref(),
        ),
        None
    );
}

#[test]
fn a_valid_harness_recovers_after_a_non_git_selection_clears_manual_state() {
    let directory = tempdir().expect("repository");
    init_repo(directory.path());
    let launch = output(directory.path(), &["rev-parse", "HEAD"]);
    let mut state = GitChangesState {
        baseline_source: BaselineSource::Manual,
        ..GitChangesState::default()
    };

    state.clear_repository("selected session is outside Git".to_owned());
    let identity = crate::worker::pty::checkout::capture(directory.path()).expect("identity");
    state.follow_session(directory.path(), &launch, &identity);
    state.refresh();

    assert_eq!(state.baseline_source, BaselineSource::SessionLaunch);
    assert_eq!(state.baseline.as_deref(), Some(launch.as_str()));
    assert_eq!(state.error, None);
}

fn row(
    id: &str,
    label: &str,
    cwd: &Path,
    launch_commit: Option<String>,
    started_at: i64,
) -> crate::worker::pty::SessionRow {
    use crate::worker::pty::{PtyState, SessionControl, SessionRow};
    SessionRow {
        id: id.to_owned(),
        label: label.to_owned(),
        provider: medulla::protocol::HarnessProvider::Codex,
        state: PtyState::Running,
        cwd: cwd.to_string_lossy().into_owned(),
        branch: None,
        launch_root: repository::discover_in(cwd)
            .ok()
            .map(|(root, _)| root.to_string_lossy().into_owned()),
        launch_commit,
        launch_checkout_identity: crate::worker::pty::checkout::capture(cwd),
        session_id: None,
        thread_name: None,
        started_at,
        last_output_at: started_at,
        last_error: None,
        busy: false,
        control: SessionControl::User,
        origin: crate::worker::pty::SessionOrigin::User,
        name: None,
        attention: None,
    }
}

fn init_repo(root: &Path) {
    git(root, &["init"]);
    configure_repo(root);
    git(root, &["commit", "--allow-empty", "-m", "baseline"]);
}

fn configure_repo(root: &Path) {
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test"]);
}

fn git(root: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .expect("git")
        .success());
}

fn output(root: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("git");
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}
