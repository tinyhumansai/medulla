//! Status-line connection-dot coverage: the dot beside the backend host reports
//! the stream's health whether or not a cycle is running — a connection that
//! dropped while idle is exactly the thing worth seeing.

use crate::helpers::*;

#[test]
fn an_idle_runtime_still_reports_its_connection() {
    let mut app = app_with_workers(Some(StreamState::Resyncing));
    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("◌ api.tinyhumans.ai"),
        "idle status line still shows the connection dot: {out}"
    );
}

#[test]
fn header_shows_stream_glyph_for_running_cycle() {
    // A dedicated runtime whose snapshot reports running=true, with a Stalled stream.
    struct RunningFleet(MockRuntime);
    impl RunningFleet {
        fn new() -> Self {
            let m = MockRuntime::empty();
            m.set_running(true);
            RunningFleet(m)
        }
    }
    impl Runtime for RunningFleet {
        fn snapshot(&self) -> RuntimeSnapshot {
            self.0.snapshot()
        }
        fn subscribe(&self) -> broadcast::Receiver<()> {
            self.0.subscribe()
        }
        fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>> {
            self.0.submit(input)
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
        fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>> {
            self.0.list_main_chats()
        }
        fn resume_chat(&self, id: String) -> BoxFuture<'static, anyhow::Result<()>> {
            self.0.resume_chat(id)
        }
        fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
            self.0.inspect_context()
        }
        fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
            self.0.shutdown()
        }
        fn stream_state(&self) -> Option<StreamState> {
            Some(StreamState::Stalled)
        }
    }

    let rt: Arc<dyn Runtime> = Arc::new(RunningFleet::new());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    let out = render(&mut app, 120, 40);
    assert!(
        out.contains("✕ api.tinyhumans.ai"),
        "a stalled stream shows a stalled dot: {out}"
    );
}
