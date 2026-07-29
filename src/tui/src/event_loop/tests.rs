//! Deterministic tests for the event loop's asynchronous command dispatcher.

use std::sync::Arc;
use std::time::Duration;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{Runtime, WorkerOp};
use medulla_tui::ui::app::{App, Cmd};

use super::cmd_dispatch::run_cmd;
use super::should_refresh_context;
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

#[tokio::test]
async fn dispatches_conversation_fleet_usage_and_context_commands() {
    let concrete = Arc::new(MockRuntime::demo());
    let runtime: Arc<dyn Runtime> = concrete.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = medulla::config::WorkflowsConfig::default();

    run_cmd(Cmd::Quit, &runtime, &cfg, &tx);
    assert!(rx.try_recv().is_err());

    run_cmd(Cmd::Submit("hello".into()), &runtime, &cfg, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::Status(s) if s == "Cycle complete"));

    run_cmd(Cmd::Resume("tui-demo-1".into()), &runtime, &cfg, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::Resumed(s) if s == "Resumed chat"));

    run_cmd(Cmd::ListChats, &runtime, &cfg, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::OpenResume(chats) if chats.len() == 2));

    run_cmd(Cmd::InspectContext, &runtime, &cfg, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::Contexts(items) if items.len() == 2));

    run_cmd(
        Cmd::WorkerOp(WorkerOp::Add {
            address: Some("peer-1".into()),
            handle: None,
            label: Some("Peer".into()),
            harness: None,
        }),
        &runtime,
        &cfg,
        &tx,
    );
    assert!(matches!(next(&mut rx).await, AppMsg::Status(s) if s == "Worker registry updated"));

    run_cmd(Cmd::LoadUsage, &runtime, &cfg, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::UsageLoaded(None)));
}

#[test]
fn context_refresh_tracks_the_nested_settings_page() {
    let runtime = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));

    let _ = app.focus_settings_subpage("Usage");
    assert!(!should_refresh_context(&mut app));
    let _ = app.focus_settings_subpage("Context");
    assert!(should_refresh_context(&mut app));
    assert!(!should_refresh_context(&mut app));
}

#[tokio::test]
async fn dispatcher_surfaces_resume_errors() {
    let concrete = Arc::new(MockRuntime::demo());
    let runtime: Arc<dyn Runtime> = concrete.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = medulla::config::WorkflowsConfig::default();

    concrete.set_running(true);
    run_cmd(Cmd::Resume("any".into()), &runtime, &cfg, &tx);
    assert!(matches!(next(&mut rx).await, AppMsg::Status(s) if s.contains("cannot resume")));
    concrete.set_running(false);
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

/// A runtime whose `submit` returns on acceptance rather than on completion,
/// like the embedded core's non-blocking `send_message`.
///
/// Delegates everything else to [`MockRuntime`]; the only interesting method is
/// [`Runtime::submit_settles_cycle`].
struct AcceptsWithoutSettling(Arc<MockRuntime>);

impl Runtime for AcceptsWithoutSettling {
    fn describe(&self) -> String {
        self.0.describe()
    }
    fn snapshot(&self) -> medulla::runtime::RuntimeSnapshot {
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
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Vec<medulla::runtime::ContextItem>>>
    {
        self.0.inspect_context()
    }
    fn shutdown(&self) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
        self.0.shutdown()
    }
}

#[tokio::test]
async fn a_non_blocking_submit_reports_acceptance_not_completion() {
    // Claiming "Cycle complete" the moment a non-blocking wire accepts the
    // message tells the operator the turn is done while it is still producing.
    let runtime: Arc<dyn Runtime> = Arc::new(AcceptsWithoutSettling(Arc::new(MockRuntime::demo())));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = medulla::config::WorkflowsConfig::default();

    run_cmd(Cmd::Submit("hello".into()), &runtime, &cfg, &tx);
    assert!(matches!(
        next(&mut rx).await,
        AppMsg::Status(s) if s.starts_with("Sent")
    ));
}
