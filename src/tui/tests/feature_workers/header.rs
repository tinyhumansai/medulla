//! Header stream-health indicator coverage: the glyph is suppressed while the
//! runtime is idle and surfaces the stream state only while a cycle is running.

use crate::helpers::*;

#[test]
fn idle_header_omits_stream_health() {
    // Stream health only shows while a cycle runs; the demo mock is idle.
    let mut app = app_with_workers(Some(StreamState::Resyncing));
    let out = render(&mut app, 120, 40);
    assert!(
        !out.contains("resyncing"),
        "idle header omits stream health"
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
        fn fork(&self, name: Option<String>) -> String {
            self.0.fork(name)
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
        fn set_async_mode(&self, on: bool) -> bool {
            self.0.set_async_mode(on)
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
        out.contains("stalled"),
        "running header shows stream health"
    );
}
