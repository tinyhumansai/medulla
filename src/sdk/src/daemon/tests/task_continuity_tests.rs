//! Conversation continuity: which task frames resume a prior session, which
//! stay separate, and — the case a race would silently break — that two
//! frames naming the *same* conversation never plan or run concurrently.
//!
//! Split out of [`super::task_tests`] once this group pushed that file over
//! the repository's 500-line file ceiling.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{mpsc, Notify};

use crate::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use crate::daemon::DaemonRuntime;
use crate::sessions::WorkspaceContext;

use super::{base_config, decoded_frames, recording_send, resume_runner, task_frame, wait_ready};

/// A task frame naming a continuity group.
fn conversation_frame(task_id: &str, text: &str, conversation: &str) -> crate::protocol::TaskFrame {
    crate::protocol::TaskFrame {
        conversation: Some(conversation.to_string()),
        fleet_depth: 0,
        ..task_frame(task_id, text, None)
    }
}

#[tokio::test]
async fn an_ordinary_task_resumes_nothing() {
    // The default, and the invariant the whole feature is built around: a task
    // frame is discrete work, so two tasks must never see each other's context.
    let resumed = Arc::new(StdMutex::new(Vec::new()));
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        base_config(),
        resume_runner(resumed.clone(), "sess-1"),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "a", None)),
    );
    runtime.idle().await;
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t2", "b", None)),
    );
    runtime.idle().await;

    assert_eq!(resumed.lock().unwrap().clone(), vec![None, None]);
}

#[tokio::test]
async fn a_second_task_in_one_conversation_resumes_the_first_ones_session() {
    // What makes the copilot pane a chat rather than a series of unrelated
    // requests: the second instruction reaches the session that ran the first.
    let resumed = Arc::new(StdMutex::new(Vec::new()));
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        base_config(),
        resume_runner(resumed.clone(), "sess-1"),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(conversation_frame("t1", "add a step", "pane-1")),
    );
    runtime.idle().await;
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(conversation_frame("t2", "now the other one", "pane-1")),
    );
    runtime.idle().await;

    assert_eq!(
        resumed.lock().unwrap().clone(),
        vec![None, Some("sess-1".to_string())],
        "the first turn opens a session; the second continues it"
    );
}

#[tokio::test]
async fn a_resumed_task_restores_the_bound_sessions_workspace_context() {
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let runner: RunTaskFn = {
        let seen = seen.clone();
        Arc::new(move |opts: RunTaskOptions| {
            seen.lock().unwrap().push(opts.workspace_context.clone());
            Box::pin(async move {
                if let Some(on_workspace_context) = opts.on_workspace_context {
                    on_workspace_context(WorkspaceContext {
                        cwd: Some("/repo/worktrees/pr-153".to_string()),
                        branch: Some("fix/pr-context".to_string()),
                        pull_request: Some("https://github.com/acme/repo/pull/153".to_string()),
                    });
                }
                if let Some(on_session) = opts.on_session {
                    on_session("sess-1".to_string());
                }
                Ok(RunTaskResult {
                    session_id: Some("sess-1".to_string()),
                    usage: None,
                    provider: opts.provider,
                    reply: "done".to_string(),
                    events: 0,
                })
            })
        })
    };
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), runner, send);

    for (task_id, text) in [("t1", "open the PR"), ("t2", "update the PR")] {
        runtime.handle_message(
            "peer".into(),
            String::new(),
            Some(conversation_frame(task_id, text, "pane-1")),
        );
        runtime.idle().await;
    }

    assert_eq!(seen.lock().unwrap()[0], WorkspaceContext::default());
    assert_eq!(
        seen.lock().unwrap()[1].cwd.as_deref(),
        Some("/repo/worktrees/pr-153"),
        "the resumed mapper must start in the checkout selected by the first turn"
    );
    let replies = decoded_frames(&recorded)
        .into_iter()
        .filter(|frame| frame.kind == crate::protocol::TaskFrameKind::Reply)
        .collect::<Vec<_>>();
    assert_eq!(
        replies[1]
            .work
            .as_ref()
            .and_then(|work| work.info.pull_request.as_deref()),
        Some("https://github.com/acme/repo/pull/153"),
        "the resumed task snapshot must immediately retain its session PR"
    );
}

#[tokio::test]
async fn two_conversations_from_one_peer_stay_separate() {
    // Two workflows open side by side are two panes, and an operator does not
    // expect an instruction about one to land in the other's context.
    let resumed = Arc::new(StdMutex::new(Vec::new()));
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        base_config(),
        resume_runner(resumed.clone(), "sess-1"),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(conversation_frame("t1", "a", "pane-1")),
    );
    runtime.idle().await;
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(conversation_frame("t2", "b", "pane-2")),
    );
    runtime.idle().await;

    assert_eq!(resumed.lock().unwrap().clone(), vec![None, None]);
}

#[tokio::test]
async fn one_peer_cannot_resume_another_peers_conversation() {
    // The frame body cannot be trusted to name its own author, so the
    // conversation is scoped to the authenticated sender. Without that scoping
    // any peer could name "pane-1" and resume into a session holding someone
    // else's context.
    let resumed = Arc::new(StdMutex::new(Vec::new()));
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        base_config(),
        resume_runner(resumed.clone(), "sess-1"),
        send,
    );

    runtime.handle_message(
        "peer-alice".into(),
        String::new(),
        Some(conversation_frame("t1", "a", "pane-1")),
    );
    runtime.idle().await;
    runtime.handle_message(
        "peer-mallory".into(),
        String::new(),
        Some(conversation_frame("t2", "b", "pane-1")),
    );
    runtime.idle().await;

    assert_eq!(resumed.lock().unwrap().clone(), vec![None, None]);
}

/// A runner that records the order calls actually *entered* the harness,
/// signals its own readiness, then blocks on a shared gate — so a test can
/// hold one call open while asserting whether a second one was let through.
fn ordered_gated_runner(
    order: Arc<StdMutex<Vec<String>>>,
    ready: mpsc::UnboundedSender<()>,
    gate: Arc<Notify>,
) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        let order = order.clone();
        let ready = ready.clone();
        let gate = gate.clone();
        Box::pin(async move {
            // Distinguishable per call so a test can name which session it is.
            let session_id = format!("sess-{}", order.lock().unwrap().len() + 1);
            order.lock().unwrap().push(opts.prompt.clone());
            let _ = ready.send(());
            gate.notified().await;
            if let Some(on_session) = opts.on_session {
                on_session(session_id.clone());
            }
            Ok(RunTaskResult {
                session_id: Some(session_id),
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    })
}

#[tokio::test]
async fn two_frames_in_one_conversation_never_overlap_in_the_harness() {
    // Regression for the race `SessionRegistry::acquire_turn` exists to close:
    // two frames naming the same conversation must never both `plan()` (and
    // therefore run) before the first has recorded its session. Before the
    // fix, `handle_task` never acquired that guard, so a second frame in the
    // same conversation could enter the harness while the first was still
    // open — planning against the same "nothing bound yet" snapshot, then
    // racing to bind whichever session finished last.
    let (send, _recorded) = recording_send();
    let order = Arc::new(StdMutex::new(Vec::new()));
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let runtime = DaemonRuntime::new(
        base_config(),
        ordered_gated_runner(order.clone(), ready_tx, gate.clone()),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(conversation_frame("t1", "a", "pane-1")),
    );
    // t1 has entered the runner and is now blocked on the gate.
    wait_ready(&mut ready_rx).await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(conversation_frame("t2", "b", "pane-1")),
    );
    // Give a wrongly-unserialized t2 every chance to race in before asserting
    // it did not: a flaky pass here would just mean the timing window was
    // missed, never that the property holds.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        order.lock().unwrap().clone(),
        vec!["a".to_string()],
        "t2 must not enter the harness while t1's turn on this conversation is still open"
    );

    // Release t1: it records its session and returns, which is what frees the
    // turn chain for t2.
    gate.notify_one();
    wait_ready(&mut ready_rx).await;
    gate.notify_one();
    runtime.idle().await;

    assert_eq!(
        order.lock().unwrap().clone(),
        vec!["a".to_string(), "b".to_string()],
        "both ran, strictly one after the other"
    );
}

#[tokio::test]
async fn the_reply_names_the_session_that_served_the_task() {
    // The worker is the only party that knows which session ran the work, and
    // the terminal frame is the only place it can say so. Without this the id
    // never leaves this process and nothing upstream can point at where a task
    // actually happened.
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        base_config(),
        resume_runner(Arc::new(StdMutex::new(Vec::new())), "sess-1"),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "audit", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let reply = frames
        .iter()
        .find(|frame| frame.kind == crate::protocol::TaskFrameKind::Reply)
        .expect("a terminal reply");
    assert_eq!(reply.session_id.as_deref(), Some("sess-1"));

    // Only the terminal frame claims it. An ack is sent before any session
    // exists, and a status frame describes progress, not placement.
    for frame in frames
        .iter()
        .filter(|frame| frame.kind != crate::protocol::TaskFrameKind::Reply)
    {
        assert_eq!(
            frame.session_id, None,
            "{:?} must not claim a session",
            frame.kind
        );
    }
}
