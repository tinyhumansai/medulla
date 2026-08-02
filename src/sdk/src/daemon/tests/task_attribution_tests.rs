//! Which sender a run and its logging are attributed to.
//!
//! Split out of [`super::task_tests`] once this group pushed that file over
//! the repository's 500-line file ceiling.

use std::sync::{Arc, Mutex as StdMutex};

use crate::daemon::DaemonRuntime;

use super::{base_config, conversation_runner, recording_send, task_frame};

#[tokio::test]
async fn a_run_is_attributed_to_the_authenticated_sender() {
    // The executor decides which session serves a task and whose context it may
    // see, entirely from this field. If it ever silently defaults to empty, two
    // peers collapse into one conversation — so it is pinned here rather than
    // trusted to stay wired.
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), conversation_runner(seen.clone()), send);

    runtime.handle_message(
        "peer-alice".to_string(),
        String::new(),
        Some(task_frame("t1", "do it", None)),
    );
    runtime.idle().await;

    assert_eq!(seen.lock().unwrap().clone(), vec!["peer-alice".to_string()]);
}

#[tokio::test]
async fn a_plain_text_dm_is_attributed_to_its_sender_too() {
    // Plain text routes to a *conversation*, which is meaningless without
    // knowing whose it is.
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), conversation_runner(seen.clone()), send);

    runtime.handle_message("peer-bob".to_string(), "hello there".to_string(), None);
    runtime.idle().await;

    assert_eq!(seen.lock().unwrap().clone(), vec!["peer-bob".to_string()]);
}

#[tokio::test]
async fn two_peers_are_never_attributed_to_one_conversation() {
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), conversation_runner(seen.clone()), send);

    runtime.handle_message(
        "peer-alice".to_string(),
        String::new(),
        Some(task_frame("t1", "a", None)),
    );
    runtime.handle_message(
        "peer-bob".to_string(),
        String::new(),
        Some(task_frame("t2", "b", None)),
    );
    runtime.idle().await;

    let mut got = seen.lock().unwrap().clone();
    got.sort();
    assert_eq!(got, vec!["peer-alice".to_string(), "peer-bob".to_string()]);
}

#[tokio::test]
async fn the_captured_turn_and_the_sent_payload_are_both_logged() {
    // These are two different facts and they used to share one line. A harness
    // that answered with nothing and a send that never happened both showed up
    // as "task ✓", so the operator could not tell which end had failed — the
    // question that took several rounds to answer in the field.
    let seen: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        base_config(),
        conversation_runner(Arc::new(StdMutex::new(Vec::new()))),
        send,
    )
    .with_log({
        let seen = seen.clone();
        Arc::new(move |line: &str| seen.lock().unwrap().push(line.to_string()))
    });

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "do it", None)),
    );
    // The turn is spawned; wait for the terminal frame to have been narrated.
    // Bounded, because a test that hangs when the line never comes is worse
    // than one that fails: it reports nothing and blocks the suite.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let lines = loop {
        let lines = seen.lock().unwrap().clone();
        if lines.iter().any(|l| l.contains("bytes on the wire")) {
            break lines;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the sent payload was never narrated; got {lines:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    let captured = lines
        .iter()
        .find(|l| l.contains("captured"))
        .unwrap_or_else(|| panic!("the harness's own output must be logged: {lines:?}"));
    assert!(captured.contains("done"), "got {captured}");
    assert!(captured.contains("4 chars"), "got {captured}");

    let sent = lines
        .iter()
        .find(|l| l.contains("bytes on the wire"))
        .unwrap_or_else(|| panic!("the payload sent to the peer must be logged: {lines:?}"));
    assert!(sent.contains("peer"), "the recipient is named: {sent}");
    assert!(sent.contains("done"), "got {sent}");
}
