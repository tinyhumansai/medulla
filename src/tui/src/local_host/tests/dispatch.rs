//! Which executor a task reaches — the watchable PTY one or the headless
//! fallback — per harness.

use std::collections::HashMap;

use medulla::daemon::providers::{Abort, RunTaskOptions};
use medulla::protocol::HarnessProvider;
use medulla_tui::worker::executor::PtySessionExecutor;
use medulla_tui::worker::pty::PtyManager;

use crate::local_host::run_task;

/// Bare-bones `RunTaskOptions` for a dispatch test — no callbacks, no
/// conversation, just enough to reach the executor the dispatcher picks.
fn dispatch_options(provider: HarnessProvider, bin_env_key: &str) -> RunTaskOptions {
    RunTaskOptions {
        provider,
        prompt: "hi".to_string(),
        cwd: ".".to_string(),
        env: HashMap::from([(
            bin_env_key.to_string(),
            "/definitely/does/not/exist-medulla-test".to_string(),
        )]),
        timeout_ms: 5_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        conversation: String::new(),
        session_class: medulla::sessions::SessionClass::Bounded,
        resume_session_id: None,
        workspace_context: Default::default(),
        abort: Abort::new(),
        router: None,
        on_event: None,
        on_stdin: None,
        on_session: None,
        on_workspace_context: None,
        attribution: true,
    }
}

#[tokio::test]
async fn opencode_falls_back_to_the_headless_executor_rather_than_being_refused() {
    // The regression: switching the local host to `PtySessionExecutor` made
    // every OpenCode task fail with "cannot run watchable tasks", because that
    // executor refuses a provider it has no transcript to tail for — even
    // though OpenCode is fully capable of running headlessly, exactly as it did
    // before this host existed. The dispatcher must route it there instead of
    // leaving it stranded behind the pty refusal.
    let run = run_task(PtySessionExecutor::new(
        PtyManager::new(),
        HashMap::new(),
        ".".to_string(),
    ));

    let error = run(dispatch_options(
        HarnessProvider::Opencode,
        "TINYPLACE_OPENCODE_BIN",
    ))
    .await
    .expect_err("a nonexistent binary must fail to spawn");

    assert!(
        !error.contains("cannot run watchable tasks"),
        "opencode must never hit the pty executor's refusal: {error}"
    );
    // The headless executor's own failure-to-spawn message, proving the task
    // actually reached `run_provider_task` rather than merely avoiding the pty
    // one for an unrelated reason.
    assert!(
        error.contains("failed to start"),
        "expected the headless executor's spawn error, got: {error}"
    );
}

#[tokio::test]
async fn claude_and_codex_still_reach_the_pty_executor() {
    // The other half of the dispatch: providers the pty executor *can* tail
    // must still go through it, not accidentally fall through to headless.
    let run = run_task(PtySessionExecutor::new(
        PtyManager::new(),
        HashMap::new(),
        ".".to_string(),
    ));

    for provider in [HarnessProvider::Claude, HarnessProvider::Codex] {
        let error = run(dispatch_options(provider, "irrelevant"))
            .await
            .expect_err("no real harness is installed in this test env");
        assert!(
            !error.contains("failed to start"),
            "{provider:?} must not take the headless path: {error}"
        );
    }
}
