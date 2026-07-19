//! Per-thread SSE task wiring: spawn the loop that streams a backend session's
//! events and folds each one into shared [`State`], and attach or replace that
//! task on a given thread.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::client::MedullaClient;

use super::types::State;

/// Spawn the per-thread SSE loop: fold each envelope and ping after every fold.
pub(super) fn spawn_stream(
    client: MedullaClient,
    state: Arc<Mutex<State>>,
    tx: broadcast::Sender<()>,
    session_id: String,
    cursor: Option<u64>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let stream = client.stream_events(&session_id, cursor);
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            if let Ok(env) = item {
                {
                    let mut s = state.lock().unwrap();
                    s.fold(&session_id, &env);
                }
                // Ping after every fold so the UI re-pulls a snapshot.
                let _ = tx.send(());
            }
            // Transient stream errors are swallowed; the client stream
            // reconnects internally from its cursor.
        }
    })
}

/// Attach a fresh SSE task to the thread `thread_id`, replacing any prior one.
pub(super) fn start_stream_on(
    client: &MedullaClient,
    state: &Arc<Mutex<State>>,
    tx: &broadcast::Sender<()>,
    thread_id: &str,
    cursor: Option<u64>,
) {
    let mut s = state.lock().unwrap();
    let Some(t) = s.by_id(thread_id) else {
        return;
    };
    if t.session_id.is_empty() {
        return;
    }
    if let Some(h) = t.stream_task.take() {
        h.abort();
    }
    let session_id = t.session_id.clone();
    let handle = spawn_stream(
        client.clone(),
        state.clone(),
        tx.clone(),
        session_id,
        cursor,
    );
    if let Some(t) = s.by_id(thread_id) {
        t.stream_task = Some(handle);
    }
}
