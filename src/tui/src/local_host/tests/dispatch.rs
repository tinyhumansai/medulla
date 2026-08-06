//! Which executor a task reaches — the watchable PTY one or the headless
//! fallback — per harness, and the directory it reaches it in.

use std::collections::HashMap;

use medulla::bridge::LocalBridgeNetwork;
use medulla::config::HostSection;
use medulla::daemon::providers::{Abort, RunTaskOptions};
use medulla::protocol::HarnessProvider;
use medulla::runtime::AgentDeclaration;
use medulla_tui::worker::executor::PtySessionExecutor;
use medulla_tui::worker::pty::PtyManager;

use crate::local_host::run_task;

use super::env_with_only_claude;

/// Bare-bones `RunTaskOptions` for a dispatch test — no callbacks, no
/// conversation, just enough to reach the executor the dispatcher picks.
fn dispatch_options(provider: HarnessProvider, bin_env_key: &str) -> RunTaskOptions {
    RunTaskOptions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        transport: Default::default(),
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
        "MEDULLA_OPENCODE_BIN",
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

/// The dispatch a declaration cannot describe.
///
/// A host binds one address and serves every agent on it from the one executor
/// it started in its own workspace; a task frame names a task, a prompt and at
/// most a harness, never which agent was selected. So two agents declared on one
/// host for two different repositories would both run in the host's directory
/// while advertising two — and the orchestrator's placement, the operator's
/// mental model, and the files that actually change would all disagree.
///
/// Until the wire carries the selected agent's id, the second declaration is
/// refused at start-up rather than silently served from the wrong checkout.
#[tokio::test]
async fn two_agents_on_one_host_cannot_claim_two_workspaces() {
    let network = LocalBridgeNetwork::new();
    let config = HostSection::default();
    let env = env_with_only_claude();
    let options = crate::local_host::options_from_config(
        &config,
        &env,
        None,
        None,
        None,
        &crate::local_host::LaunchPolicy {
            attribution: true,
            ..Default::default()
        },
    )
    .expect("valid config");
    let here = medulla::daemon::embedded::resolve_workspace("");
    let elsewhere = if cfg!(windows) {
        "C:\\srv\\web"
    } else {
        "/srv/web"
    };

    let error = crate::local_host::start(
        &config,
        &HashMap::new(),
        &network,
        options,
        PtyManager::new(),
        &[
            AgentDeclaration::new("api-claude", "this-device", "claude", here.clone()),
            AgentDeclaration::new("web-claude", "this-device", "claude", elsewhere),
        ],
    )
    .expect_err("one host cannot run two workspaces");

    assert!(
        error.contains("web-claude") && error.contains(elsewhere) && error.contains(&here),
        "the refusal must name the agent and both directories: {error}"
    );
    assert!(
        error.contains("[[hosts]]"),
        "…and the way to actually get a second workspace: {error}"
    );
}
