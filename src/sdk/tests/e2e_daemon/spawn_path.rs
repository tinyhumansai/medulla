//! Daemon e2e over the REAL spawn path ([`run_provider_task`]) driving fake
//! provider CLIs (shell scripts emitting realistic JSONL) via `TINYPLACE_*_BIN`
//! overrides: task lifecycle, tool-status mapping, mid-run stdin forwarding,
//! capabilities merge + digest fallback, plaintext DMs, and the idle watchdog.

use crate::helpers::*;
use crate::support::fake_provider::{
    claude_capabilities_script, claude_capabilities_script_without_summary, claude_script,
    claude_stdin_echo_script, hanging_script, opencode_script, provider_env, TempDir,
};
use crate::support::wait_until;

// 1. Task frame lifecycle e2e via the real spawn path (claude).
//    ack("task accepted") → status* → reply, correlationId echoed, harness set,
//    reply text from the claude result line (precedence over the agent message).
#[tokio::test]
async fn task_lifecycle_ack_status_reply() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &claude_script("intermediate", "final answer"));
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "cyc-1", "do it", Some("corr-1"))),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    assert!(!frames.is_empty());
    // First frame is the ack, last is the reply; the middle are all statuses.
    assert_eq!(frames[0].kind, TaskFrameKind::Ack);
    assert_eq!(frames[0].text, "task accepted");
    let last = frames.last().unwrap();
    assert_eq!(last.kind, TaskFrameKind::Reply);
    assert_eq!(
        last.text, "final answer",
        "claude result line wins over agent message"
    );
    let usage = last.usage.expect("reply carries the child's token usage");
    assert_eq!(usage.input_tokens, 1234);
    assert_eq!(usage.output_tokens, 56);
    for middle in &frames[1..frames.len() - 1] {
        assert_eq!(
            middle.kind,
            TaskFrameKind::Status,
            "middle frames are status"
        );
    }
    assert!(
        frames.iter().any(|f| f.kind == TaskFrameKind::Status),
        "at least one status frame"
    );
    // correlationId echoed + harness set on every frame.
    for f in &frames {
        assert_eq!(f.correlation_id.as_deref(), Some("corr-1"));
        assert_eq!(f.harness, Some(HarnessProvider::Claude));
    }
}

// 2. Fake provider via real spawn (opencode): tool_call/tool_result mapping into
//    status details, and the final text part as the reply.
#[tokio::test]
async fn opencode_tool_status_details_and_reply() {
    let tmp = TempDir::new();
    let script = tmp.write_script("opencode", &opencode_script("opencode done"));
    let env = provider_env(&[("TINYPLACE_OPENCODE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Opencode, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "oc-1", "read a file", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let statuses: Vec<String> = frames
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Status)
        .map(|f| f.text.clone())
        .collect();
    assert!(
        statuses.iter().any(|s| s == "running read: /a/b.rs"),
        "tool_call → status: {statuses:?}"
    );
    assert!(
        statuses.iter().any(|s| s == "tool completed"),
        "tool_result → status: {statuses:?}"
    );
    let reply = frames.last().unwrap();
    assert_eq!(reply.kind, TaskFrameKind::Reply);
    assert_eq!(reply.text, "opencode done");
    assert_eq!(reply.harness, Some(HarnessProvider::Opencode));
}

// 3. Input forwarding: a stdin-reading fake provider gets a mid-task `input`
//    frame; the daemon acks it and the text reaches the child (visible in reply).
#[tokio::test]
async fn input_frame_reaches_provider_stdin() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &claude_stdin_echo_script());
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "in-1", "start", Some("c9"))),
    );
    // Wait for the task ack (the running record exists by then, so input buffers).
    wait_until("task accepted", T, || {
        decoded_frames(&recorded)
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted")
    })
    .await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(
            TaskFrameKind::Input,
            "in-1",
            "extra guidance",
            Some("c9"),
        )),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    assert!(
        frames
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.text == "input received"),
        "input should be acked"
    );
    let reply = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Reply)
        .expect("reply");
    assert_eq!(
        reply.text, "got: extra guidance",
        "forwarded input reached the provider"
    );
}

// 5. Capabilities: a `capabilities` frame → capabilities_result with parseable
//    AgentCapabilities; daemon cwd wins over the report, tools come from report.
#[tokio::test]
async fn capabilities_result_merges_daemon_and_report() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &claude_capabilities_script());
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(
            TaskFrameKind::Capabilities,
            "cap-1",
            "",
            Some("corr-9"),
        )),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let result = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::CapabilitiesResult)
        .expect("capabilities_result frame");
    assert_eq!(result.correlation_id.as_deref(), Some("corr-9"));
    assert_eq!(result.harness, Some(HarnessProvider::Claude));

    let caps = parse_agent_capabilities(&result.text).expect("parseable capabilities");
    // cwd from the daemon (canonicalized workspace) wins.
    let expected_cwd = std::fs::canonicalize(tmp.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(caps.cwd.as_deref(), Some(expected_cwd.as_str()));
    // tools/mcpServers come from the report.
    assert_eq!(caps.tools, vec!["Bash", "Read"]);
    assert_eq!(caps.mcp_servers, vec!["langfuse"]);
    // accessibleDirs unions the daemon cwd with the reported dir.
    assert!(caps.accessible_dirs.contains(&expected_cwd));
    assert!(caps.accessible_dirs.iter().any(|d| d == "/opt/extra"));
    assert_eq!(caps.summary.as_deref(), Some("fake claude"));
}

// 5b. Capabilities: when the agent's report has no `summary`, the workspace's
//     README.md digest back-fills it in the capabilities_result frame.
#[tokio::test]
async fn capabilities_summary_falls_back_to_workspace_digest() {
    let tmp = TempDir::new();
    std::fs::write(
        tmp.path().join("README.md"),
        "# Widget\n\nA widget library.",
    )
    .unwrap();
    let script = tmp.write_script("claude", &claude_capabilities_script_without_summary());
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(
            TaskFrameKind::Capabilities,
            "cap-2",
            "",
            Some("corr-10"),
        )),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let result = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::CapabilitiesResult)
        .expect("capabilities_result frame");
    let caps = parse_agent_capabilities(&result.text).expect("parseable capabilities");
    assert_eq!(caps.tools, vec!["Bash"]);
    assert_eq!(
        caps.summary.as_deref(),
        Some("README.md: Widget — A widget library.")
    );
}

// 6. Plaintext DM: a non-frame body → raw text reply (no frame) via real spawn.
#[tokio::test]
async fn plaintext_dm_replies_raw() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &claude_script("ignored", "plain reply"));
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message("peer".into(), "just chatting".into(), None);
    runtime.idle().await;

    let bodies = raw_bodies(&recorded);
    assert!(
        bodies.iter().any(|b| b == "plain reply"),
        "raw reply: {bodies:?}"
    );
    // The reply is not a protocol frame.
    for body in &bodies {
        assert!(
            decode_task_frame(body).is_none(),
            "plaintext reply must not be a frame"
        );
    }
}

// 7. Idle watchdog: a hanging fake provider with a tiny task timeout → error frame
//    mentioning idle; the child is killed so the test finishes fast.
#[tokio::test]
async fn idle_watchdog_kills_hung_provider() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &hanging_script());
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let mut cfg = config(HarnessProvider::Claude, tmp.path_str(), env);
    cfg.task_timeout_ms = 300; // fire fast
    let runtime = DaemonRuntime::new(cfg, real_run_task(), send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "hang-1", "wait forever", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let error = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Error)
        .expect("idle error frame");
    assert!(error.text.contains("idle"), "got: {}", error.text);
}
