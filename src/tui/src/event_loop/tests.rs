//! Deterministic tests for the event loop's asynchronous command dispatcher.

use std::sync::Arc;
use std::time::Duration;

use medulla::client::{FeedbackQuery, FeedbackType};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{Runtime, WorkerOp};
use medulla_tui::ui::app::Cmd;

use super::cmd_dispatch::{read_memory, run_cmd};
use super::types::AppMsg;
use super::update_checker::spawn_update_checker;

/// Receive the next dispatcher result without allowing a broken task to hang
/// the entire test suite.
async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppMsg>) -> AppMsg {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("dispatcher timed out")
        .expect("dispatcher dropped its response channel")
}

fn git(root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn dispatches_workspace_load_diff_and_exact_commit() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.name", "TUI Test"]);
    git(dir.path(), &["config", "user.email", "tui@example.invalid"]);
    std::fs::write(dir.path().join("file.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "file.txt"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    std::fs::write(dir.path().join("file.txt"), "two\n").unwrap();

    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    run_cmd(
        Cmd::LoadWorkspaces(vec![dir.path().to_path_buf()]),
        &runtime,
        None,
        &tx,
    );
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::WorkspacesLoaded(reports)
            if reports.len() == 1 && reports[0].snapshot.is_some()
    ));
    run_cmd(
        Cmd::LoadWorkspaceDiff {
            workspace: dir.path().to_path_buf(),
            path: "file.txt".into(),
        },
        &runtime,
        None,
        &tx,
    );
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::WorkspaceDiffLoaded { result: Ok(diff), .. } if diff.contains("WORKTREE")
    ));
    run_cmd(
        Cmd::CommitPaths {
            workspace: dir.path().to_path_buf(),
            paths: vec!["file.txt".into()],
            subject: "feat(repo): commit from TUI".into(),
            shared_path_denylist: Vec::new(),
        },
        &runtime,
        None,
        &tx,
    );
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::WorkspaceCommitDone(Ok(outcome))
            if outcome.subject == "feat(repo): commit from TUI"
    ));

    std::fs::write(dir.path().join("file.txt"), "three\n").unwrap();
    run_cmd(
        Cmd::CommitPaths {
            workspace: dir.path().to_path_buf(),
            paths: vec!["file.txt".into()],
            subject: "invalid subject".into(),
            shared_path_denylist: Vec::new(),
        },
        &runtime,
        None,
        &tx,
    );
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::WorkspaceCommitDone(Err(error)) if error.contains("conventional")
    ));
}

#[tokio::test]
async fn dispatches_conversation_fleet_usage_and_context_commands() {
    let concrete = Arc::new(MockRuntime::demo());
    let runtime: Arc<dyn Runtime> = concrete.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    run_cmd(Cmd::Quit, &runtime, None, &tx);
    assert!(rx.try_recv().is_err());

    run_cmd(Cmd::Submit("hello".into()), &runtime, None, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::Status(s) if s == "Cycle complete"));

    run_cmd(Cmd::Resume("tui-demo-1".into()), &runtime, None, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::Resumed(s) if s == "Resumed chat"));

    run_cmd(Cmd::ListChats, &runtime, None, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::OpenResume(chats) if chats.len() == 2));

    run_cmd(Cmd::InspectContext, &runtime, None, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::Contexts(items) if items.len() == 2));

    run_cmd(
        Cmd::WorkerOp(WorkerOp::Add {
            address: Some("peer-1".into()),
            handle: None,
            label: Some("Peer".into()),
            harness: None,
        }),
        &runtime,
        None,
        &tx,
    );
    assert!(matches!(next(&mut rx).await, AppMsg::Status(s) if s == "Worker registry updated"));

    run_cmd(Cmd::LoadUsage, &runtime, None, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::UsageLoaded(None)));
}

#[tokio::test]
async fn prepares_exact_diff_and_submits_fresh_review_instruction() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(dir.path().join("file.txt"), "before\n").unwrap();
    git(&["add", "file.txt"]);
    git(&["commit", "-qm", "base"]);
    std::fs::write(dir.path().join("file.txt"), "after\n").unwrap();

    let concrete = Arc::new(MockRuntime::empty());
    let runtime: Arc<dyn Runtime> = concrete.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    run_cmd(
        Cmd::PrepareReview {
            task_id: "task-1".into(),
            implementer_id: "dev-1".into(),
            reviewer_id: "dev-2".into(),
            workspace: dir.path().to_path_buf(),
            contract: medulla::autoreview::ReviewContract {
                outcome: "Change the file".into(),
                non_goals: vec!["No rename".into()],
                verify: vec!["test -f file.txt".into()],
            },
        },
        &runtime,
        None,
        &tx,
    );
    assert!(
        matches!(next(&mut rx).await, AppMsg::Status(status) if status.contains("fresh reviewer"))
    );
    let message = concrete
        .snapshot()
        .messages
        .iter()
        .find(|message| message.content.contains("MEDULLA_AUTOREVIEW"))
        .unwrap()
        .content
        .clone();
    assert!(message.contains("MEDULLA_AUTOREVIEW target=task-1"));
    assert!(message.contains("-before"));
    assert!(message.contains("+after"));
    assert!(message.contains("reviewer") || message.contains("dev-2"));
}

#[tokio::test]
async fn dispatches_every_feedback_action() {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    run_cmd(
        Cmd::LoadFeedback(FeedbackQuery::default()),
        &runtime,
        None,
        &tx,
    );
    assert!(
        matches!(next(&mut rx).await, AppMsg::FeedbackLoaded(Some(page)) if !page.items.is_empty())
    );

    run_cmd(Cmd::LoadFeedbackDetail("fb-2".into()), &runtime, None, &tx);
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::FeedbackComments { id, comments } if id == "fb-2" && !comments.is_empty()
    ));

    run_cmd(
        Cmd::VoteFeedback {
            id: "fb-2".into(),
            value: 1,
        },
        &runtime,
        None,
        &tx,
    );
    assert!(matches!(next(&mut rx).await, AppMsg::FeedbackItemUpdated(item) if item.id == "fb-2"));

    run_cmd(
        Cmd::CommentFeedback {
            id: "fb-2".into(),
            body: "Useful".into(),
        },
        &runtime,
        None,
        &tx,
    );
    assert!(matches!(next(&mut rx).await, AppMsg::FeedbackChanged(s) if s.contains("comment")));

    run_cmd(
        Cmd::SubmitFeedback {
            kind: FeedbackType::Feature,
            title: "New feature".into(),
            body: "Please add it".into(),
        },
        &runtime,
        None,
        &tx,
    );
    assert!(matches!(next(&mut rx).await, AppMsg::FeedbackChanged(s) if s.contains("submitted")));
}

#[tokio::test]
async fn dispatcher_surfaces_feedback_and_resume_errors() {
    let concrete = Arc::new(MockRuntime::demo());
    let runtime: Arc<dyn Runtime> = concrete.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    for cmd in [
        Cmd::LoadFeedbackDetail("missing".into()),
        Cmd::VoteFeedback {
            id: "missing".into(),
            value: 1,
        },
        Cmd::CommentFeedback {
            id: "missing".into(),
            body: "nope".into(),
        },
    ] {
        run_cmd(cmd, &runtime, None, &tx);
        assert!(matches!(next(&mut rx).await, AppMsg::Status(s) if s.contains("not found")));
    }

    concrete.set_running(true);
    run_cmd(Cmd::Resume("any".into()), &runtime, None, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::Status(s) if s.contains("cannot resume")));
    concrete.set_running(false);
}

#[tokio::test]
async fn dispatches_runtime_memory_reads_searches_and_missing_ingest() {
    let concrete = Arc::new(MockRuntime::empty());
    concrete.set_memory_directives(vec!["Keep tests offline".into()]);
    let runtime: Arc<dyn Runtime> = concrete;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let (status, directives) = read_memory(&runtime, None);
    assert!(status.is_none());
    assert_eq!(directives, ["Keep tests offline"]);

    run_cmd(Cmd::LoadMemory, &runtime, None, &tx);
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::MemoryLoaded { status: None, directives } if directives == ["Keep tests offline"]
    ));

    run_cmd(Cmd::SearchMemory("needle".into()), &runtime, None, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::MemoryLoaded { .. }));
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::MemoryResults { hits, query } if hits.is_empty() && query == "needle"
    ));

    run_cmd(Cmd::IngestMemory { backfill: false }, &runtime, None, &tx);
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::MemoryIngestDone(s) if s.contains("no memory service")
    ));
}

#[test]
fn disabled_update_check_spawns_no_background_work() {
    let dir = tempfile::tempdir().unwrap();
    let env = std::collections::HashMap::new();
    let mut loaded = medulla::config::load_config(None, &env, dir.path()).unwrap();
    loaded.config.update.check = false;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    spawn_update_checker(&loaded, &tx);

    assert!(rx.try_recv().is_err());
}
