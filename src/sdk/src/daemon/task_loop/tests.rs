//! Unit tests for task-specific harness environment construction and for the
//! task-loop's own registration claim (concurrent duplicate rejection).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::super::tests::{base_config, recording_send};
use super::super::types::{DaemonRuntime, RunningTask};
use crate::daemon::providers::{Abort, RunTaskFn, RunTaskOptions, RunTaskResult};
use crate::protocol::HarnessProvider;

#[cfg(feature = "workflows")]
#[test]
fn clears_task_capabilities_but_preserves_operator_transport() {
    let env = std::collections::HashMap::from([
        (crate::mcp::TOOL_MODE_ENV.to_string(), "propose".to_string()),
        (
            crate::daemon::providers::HARNESS_PROTOCOL_ENV.to_string(),
            "acp".to_string(),
        ),
        (
            crate::control_socket::MCP_SOCKET_ENV.to_string(),
            "/tmp/parent.sock".to_string(),
        ),
        (
            crate::control_socket::MCP_GRANT_ENV.to_string(),
            "parent-grant".to_string(),
        ),
    ]);

    let env = super::with_tool_mode_at_depth(env, None, 0);

    assert!(!env.contains_key(crate::mcp::TOOL_MODE_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_SOCKET_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_GRANT_ENV));
    assert_eq!(
        env.get(crate::daemon::providers::HARNESS_PROTOCOL_ENV)
            .map(String::as_str),
        Some("acp")
    );
}

#[cfg(feature = "workflows")]
#[test]
fn tool_mode_forces_acp_transport() {
    let env = super::with_tool_mode_at_depth(std::collections::HashMap::new(), Some("propose"), 0);

    assert_eq!(
        env.get(crate::daemon::providers::HARNESS_PROTOCOL_ENV)
            .map(String::as_str),
        Some("acp")
    );
}

#[cfg(feature = "workflows")]
#[test]
fn delegated_full_mode_task_on_remote_worker_keeps_provider_transport() {
    let env = super::with_tool_mode_at_depth(std::collections::HashMap::new(), None, 1);

    assert!(!env.contains_key(crate::daemon::providers::HARNESS_PROTOCOL_ENV));
    assert!(!env.contains_key(crate::mcp::TOOL_MODE_ENV));
}

#[cfg(feature = "workflows")]
#[test]
fn a_verified_parent_handoff_forces_acp_without_reusing_the_parent_as_the_child() {
    let env = std::collections::HashMap::from([
        (
            crate::control_socket::MCP_PARENT_SOCKET_ENV.to_string(),
            "/tmp/parent.sock".to_string(),
        ),
        (
            crate::control_socket::MCP_PARENT_GRANT_ENV.to_string(),
            "parent-grant".to_string(),
        ),
    ]);

    let env = super::with_tool_mode_at_depth(env, None, 1);

    assert_eq!(
        env.get(crate::daemon::providers::HARNESS_PROTOCOL_ENV)
            .map(String::as_str),
        Some("acp")
    );
    assert!(!env.contains_key(crate::control_socket::MCP_SOCKET_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_GRANT_ENV));
}

#[cfg(feature = "workflows")]
#[test]
fn root_and_delegated_tasks_use_acp_when_process_owns_fleet_plane() {
    assert!(super::task_can_reach_fleet(true));
    assert!(!super::task_can_reach_fleet(false));
}

/// Duplicate rejection has to survive the two frames arriving at once.
///
/// Each frame is handled by its own spawned task, so a sequential test only
/// covers the easy ordering. With a separate `contains_key` check both racing
/// frames could pass it, the second would overwrite the first's `RunningTask`,
/// and the first admission guard's drop would then remove the shared key — a
/// still-running harness that no abort, input, or screen frame can reach.
/// Exactly one claim must win.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claims_on_one_task_key_admit_exactly_one() {
    let run_task: RunTaskFn = Arc::new(|opts: RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    });
    let (send, _recorded) = recording_send();
    let runtime = Arc::new(DaemonRuntime::new(base_config(), run_task, send));

    let claimants = 16;
    let barrier = Arc::new(tokio::sync::Barrier::new(claimants));
    let winners = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..claimants {
        let runtime = runtime.clone();
        let barrier = barrier.clone();
        let winners = winners.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            if runtime.register_running("peer|dup", running_task_stub()) {
                winners.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(
        winners.load(Ordering::SeqCst),
        1,
        "exactly one claim on a task key may be admitted"
    );
}

/// A placeholder registration record; the claim, not its contents, is the
/// subject of the test above.
fn running_task_stub() -> RunningTask {
    RunningTask {
        provider: HarnessProvider::Claude,
        accepts_stdin: false,
        correlation_id: None,
        stdin: None,
        pending_input: Vec::new(),
        session_id: None,
        abort: Abort::new(),
    }
}
