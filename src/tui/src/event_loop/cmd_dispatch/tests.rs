//! Command-dispatch coverage for every asynchronous command family.

use std::sync::Arc;
use std::time::Duration;

use medulla::client::{FeedbackQuery, FeedbackType};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{ContextItem, Runtime, RuntimeSnapshot, WorkerOp};
use medulla_tui::ui::app::Cmd;

use super::run_cmd as dispatch_cmd;
use crate::event_loop::types::AppMsg;

#[cfg(feature = "workflows")]
use super::workflows::{report_delete, DeleteOutcome};

/// Dispatch through the production entry point with default workflow settings.
fn run_cmd(
    cmd: Cmd,
    runtime: &Arc<dyn Runtime>,
    _: Option<()>,
    tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    dispatch_cmd(
        cmd,
        runtime,
        &medulla::config::TuiConfig::default(),
        tx,
        None,
    );
}

/// Receive a spawned command result through a finite test boundary.
async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppMsg>) -> AppMsg {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("dispatcher timed out")
        .expect("dispatcher dropped its response channel")
}

/// Assert the status followed by the optional catalogue-removal notification.
#[cfg(feature = "workflows")]
fn assert_delete_messages(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppMsg>,
    status_fragment: &str,
    deleted: bool,
) {
    match rx.try_recv().expect("deletion status") {
        AppMsg::Status(status) => assert!(status.contains(status_fragment), "{status}"),
        _ => panic!("deletion must report status first"),
    }
    if deleted {
        assert!(matches!(
            rx.try_recv().expect("workflow deletion notification"),
            AppMsg::WorkflowDeleted { id } if id == "sweep"
        ));
    }
    assert!(rx.try_recv().is_err(), "deletion sent an extra message");
}

#[cfg(feature = "workflows")]
#[test]
fn successful_delete_reports_catalogue_removal() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    report_delete("sweep".into(), Ok(DeleteOutcome::Deleted), &tx);

    assert_delete_messages(&mut rx, "Deleted workflow sweep", true);
}

#[cfg(feature = "workflows")]
#[test]
fn delete_with_undo_warning_still_reports_catalogue_removal() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let warning = medulla::workflows::WorkflowError::Engine("revision unavailable".into());

    report_delete(
        "sweep".into(),
        Ok(DeleteOutcome::DeletedWithWarning(warning)),
        &tx,
    );

    assert_delete_messages(&mut rx, "could not record undo history", true);
}

#[cfg(feature = "workflows")]
#[test]
fn failed_delete_does_not_report_catalogue_removal() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let failure = medulla::workflows::WorkflowError::Engine("definition retained".into());

    report_delete(
        "sweep".into(),
        Ok(DeleteOutcome::Failed(failure)),
        &tx,
    );

    assert_delete_messages(&mut rx, "Could not delete workflow sweep", false);
}

#[cfg(feature = "workflows")]
#[tokio::test]
async fn delete_task_join_failure_does_not_report_catalogue_removal() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let join_error = tokio::spawn(async { panic!("injected task failure") })
        .await
        .expect_err("panicking task must fail to join");

    report_delete("sweep".into(), Err(join_error), &tx);

    assert_delete_messages(&mut rx, "Could not delete workflow sweep", false);
}

struct FailingRuntime;

impl FailingRuntime {
    fn failure<T: Send + 'static>() -> futures::future::BoxFuture<'static, anyhow::Result<T>> {
        Box::pin(async { Err(anyhow::anyhow!("injected runtime failure")) })
    }
}

impl Runtime for FailingRuntime {
    fn describe(&self) -> String {
        "failing test runtime".into()
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot::default()
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<()> {
        let (sender, receiver) = tokio::sync::broadcast::channel(1);
        sender.send(()).unwrap();
        sender.send(()).unwrap();
        receiver
    }

    fn submit(&self, _: String) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        Self::failure()
    }

    fn abort(&self) {}

    fn new_session(&self) {}

    fn set_active_thread(&self, _: String) {}

    fn list_main_chats(
        &self,
    ) -> futures::future::BoxFuture<
        'static,
        anyhow::Result<Vec<medulla::ui::chat_store::MainChatSummary>>,
    > {
        Self::failure()
    }

    fn resume_chat(&self, _: String) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        Self::failure()
    }

    fn inspect_context(
        &self,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
        Self::failure()
    }

    fn shutdown(&self) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        Self::failure()
    }

    fn worker_op(&self, _: WorkerOp) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        Self::failure()
    }

    fn team_usage(
        &self,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Option<serde_json::Value>>> {
        Self::failure()
    }

    fn list_feedback(
        &self,
        _: FeedbackQuery,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Option<medulla::client::FeedbackPage>>>
    {
        Self::failure()
    }
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
async fn dispatches_every_feedback_action() {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    run_cmd(
        Cmd::LoadFeedback(FeedbackQuery::default()),
        &runtime,
        None,
        &tx,
    );
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::FeedbackLoaded { page: Some(page), .. } if !page.items.is_empty()
    ));

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
async fn dispatcher_surfaces_every_runtime_failure() {
    let runtime: Arc<dyn Runtime> = Arc::new(FailingRuntime);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let commands = [
        Cmd::Submit("fail".into()),
        Cmd::Resume("fail".into()),
        Cmd::ListChats,
        Cmd::InspectContext,
        Cmd::WorkerOp(WorkerOp::Select { id: "fail".into() }),
        Cmd::LoadUsage,
        Cmd::LoadFeedback(FeedbackQuery::default()),
        Cmd::LoadFeedbackDetail("fail".into()),
        Cmd::VoteFeedback {
            id: "fail".into(),
            value: 1,
        },
        Cmd::CommentFeedback {
            id: "fail".into(),
            body: "fail".into(),
        },
        Cmd::SubmitFeedback {
            kind: FeedbackType::Bug,
            title: "fail".into(),
            body: "fail".into(),
        },
    ];

    for command in commands {
        run_cmd(command, &runtime, None, &tx);
        assert!(matches!(next(&mut rx).await, AppMsg::Status(status)
                if status.contains("injected runtime failure")
                    || status.contains("requires a signed-in backend")));
    }
}

/// A runtime whose submission future signals acceptance before the cycle ends.
struct AcceptsWithoutSettling(Arc<MockRuntime>);

impl Runtime for AcceptsWithoutSettling {
    fn describe(&self) -> String {
        self.0.describe()
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        self.0.snapshot()
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.0.subscribe()
    }

    fn submit(&self, input: String) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        self.0.submit(input)
    }

    fn submit_settles_cycle(&self) -> bool {
        false
    }

    fn abort(&self) {
        self.0.abort()
    }

    fn new_session(&self) {
        self.0.new_session()
    }

    fn set_active_thread(&self, id: String) {
        self.0.set_active_thread(id)
    }

    fn list_main_chats(
        &self,
    ) -> futures::future::BoxFuture<
        'static,
        anyhow::Result<Vec<medulla::ui::chat_store::MainChatSummary>>,
    > {
        self.0.list_main_chats()
    }

    fn resume_chat(&self, id: String) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        self.0.resume_chat(id)
    }

    fn inspect_context(
        &self,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
        self.0.inspect_context()
    }

    fn shutdown(&self) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        self.0.shutdown()
    }
}

#[tokio::test]
async fn a_non_blocking_submit_reports_acceptance_not_completion() {
    let runtime: Arc<dyn Runtime> = Arc::new(AcceptsWithoutSettling(Arc::new(MockRuntime::demo())));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    run_cmd(Cmd::Submit("hello".into()), &runtime, None, &tx);
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::Status(status) if status.starts_with("Sent")
    ));
}
