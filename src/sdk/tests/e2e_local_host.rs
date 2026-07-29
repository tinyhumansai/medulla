//! End-to-end: the orchestrator half and the host half in one process.
//!
//! This is the shape a plain `medulla` runs in — a [`TaskRunner`] dispatching
//! over a [`RoutingBridge`] to an [`EmbeddedDaemon`] bound on the same
//! device-local bus. No tiny.place client, no identity, no contact graph, no
//! relay, and no second process: a task leaves the orchestrator and comes back
//! answered without a packet leaving the machine.

use std::sync::Arc;
use std::time::Duration;

use medulla::bridge::{Bridge, LocalBridgeNetwork, RoutingBridge};
use medulla::daemon::embedded::{EmbeddedDaemon, EmbeddedDaemonOptions};
use medulla::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use medulla::hub::{TaskRequest, TaskRunner};
use medulla::tinyplace::HarnessProvider;

/// The host's address on the bus, as the TUI's `[host].address` default names it.
const HOST: &str = "this-device";
/// The orchestrator's address, as `DEFAULT_LOCAL_HUB_ADDRESS` names it.
const ORCHESTRATOR: &str = "medulla-orchestrator";

/// A path that is guaranteed to exist and be executable on every platform.
///
/// The running test binary itself. `/bin/sh` was the obvious choice and the
/// wrong one: it does not exist on Windows, so detection found nothing there and
/// every test that needed an "installed" harness failed on that runner alone.
fn installed_bin() -> String {
    std::env::current_exe()
        .expect("the test binary has a path")
        .to_string_lossy()
        .into_owned()
}

/// An environment in which exactly `claude` is installed, so what the host
/// detects does not depend on what the machine running the tests happens to have.
fn env_with_only_claude() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([
        ("PATH".to_string(), String::new()),
        ("TINYPLACE_CLAUDE_BIN".to_string(), installed_bin()),
    ])
}

/// An executor that echoes the prompt back, so the assertion covers the whole
/// path the instruction travelled rather than a canned string.
fn echoing_executor() -> RunTaskFn {
    Arc::new(|options: RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                provider: options.provider,
                reply: format!("ran in {}: {}", options.cwd, options.prompt),
                events: 1,
                usage: None,
                session_id: None,
            })
        })
    })
}

#[tokio::test]
async fn a_task_round_trips_from_the_orchestrator_to_this_device_and_back() {
    let network = LocalBridgeNetwork::new();

    // The host half: what a plain `medulla` starts from its `[host]` section.
    let host = EmbeddedDaemon::start_with_executor(
        Arc::new(network.bind(HOST).unwrap()) as Arc<dyn Bridge>,
        HOST,
        EmbeddedDaemonOptions {
            env: env_with_only_claude(),
            poll: Duration::from_millis(5),
            ..Default::default()
        },
        echoing_executor(),
    )
    .expect("this device can host");

    // The orchestrator half: the hub's task sender over a routing relay. Local
    // only here — the point is that the remote transport is never needed.
    let relay = Arc::new(RoutingBridge::local_only(
        network.bind(ORCHESTRATOR).unwrap(),
    ));
    let runner = TaskRunner::start(relay, Duration::from_millis(5));

    let outcome = runner
        .run(
            TaskRequest {
                task_id: "local-1".to_string(),
                abort_id: "local-1".to_string(),
                cycle_id: Some("cycle-1".to_string()),
                instruction: "summarize the repo".to_string(),
                worker_address: HOST.to_string(),
                provider: None,
                custom_harness: None,
                model: None,
                workflow: None,
                conversation: None,
            },
            None,
        )
        .await
        .expect("the local host answers");

    assert!(
        outcome.reply.ends_with("summarize the repo"),
        "the instruction should reach the harness verbatim: {}",
        outcome.reply
    );
    assert!(
        outcome.reply.contains(host.workspace()),
        "the task should run in the host's workspace: {}",
        outcome.reply
    );
    assert_eq!(outcome.harness, Some(HarnessProvider::Claude));

    host.idle().await;
    let stats = host.stats();
    assert_eq!(stats.tasks_started, 1);
    assert_eq!(stats.tasks_completed, 1);
    assert_eq!(stats.tasks_failed, 0);
    assert_eq!(stats.last_peer.as_deref(), Some(ORCHESTRATOR));
}

#[tokio::test]
async fn concurrent_tasks_are_correlated_back_to_the_right_caller() {
    let network = LocalBridgeNetwork::new();
    let _host = EmbeddedDaemon::start_with_executor(
        Arc::new(network.bind(HOST).unwrap()) as Arc<dyn Bridge>,
        HOST,
        EmbeddedDaemonOptions {
            env: env_with_only_claude(),
            poll: Duration::from_millis(5),
            concurrency: 4,
            ..Default::default()
        },
        echoing_executor(),
    )
    .unwrap();
    let relay = Arc::new(RoutingBridge::local_only(
        network.bind(ORCHESTRATOR).unwrap(),
    ));
    let runner = Arc::new(TaskRunner::start(relay, Duration::from_millis(5)));

    // One shared, destructively-drained inbox serves every dispatch, so this is
    // the case where a correlation mistake shows up as one caller receiving
    // another's answer.
    let dispatches: Vec<_> = (0..4)
        .map(|index| {
            let runner = runner.clone();
            tokio::spawn(async move {
                runner
                    .run(
                        TaskRequest {
                            task_id: format!("task-{index}"),
                            abort_id: format!("task-{index}"),
                            cycle_id: Some("cycle".to_string()),
                            instruction: format!("instruction {index}"),
                            worker_address: HOST.to_string(),
                            provider: None,
                            custom_harness: None,
                            model: None,
                            workflow: None,
                            conversation: None,
                        },
                        None,
                    )
                    .await
                    .map(|outcome| (index, outcome.reply))
            })
        })
        .collect();

    for dispatch in dispatches {
        let (index, reply) = dispatch
            .await
            .expect("the dispatch task did not panic")
            .expect("every dispatch is answered");
        assert!(
            reply.ends_with(&format!("instruction {index}")),
            "dispatch {index} received the wrong answer: {reply}"
        );
    }
}
