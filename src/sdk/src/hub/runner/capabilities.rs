//! Lightweight worker capability probes.

use tokio::sync::oneshot;

use crate::tinyplace::{encode_task_frame, AgentCapabilities, EncodeFrameInput, TaskFrameKind};

use super::{RunError, TaskRunner, MAX_RESETS};

impl TaskRunner {
    /// Ask one worker to self-report its [`AgentCapabilities`] — the same
    /// budget/readiness-bearing snapshot the daemon answers a `capabilities`
    /// probe with.
    ///
    /// Uses the normal encrypted peer channel but never starts a harness task.
    /// The request is bounded by the runner's acknowledgement window so a stale
    /// worker cannot leave a capability advertisement pending forever; the caller
    /// (the hub's socket-plane `capabilities_result`) treats any error as "no
    /// budgets to advertise" and falls open to the static facts.
    pub async fn capabilities(&self, address: &str) -> Result<AgentCapabilities, RunError> {
        if !self.relay.contact_accepted(address).await {
            let _ = self.relay.request_contact(address).await;
            return Err(RunError::Worker(
                "worker has not accepted this hub contact yet".into(),
            ));
        }
        let mut attempt = 0;
        loop {
            let correlation_id = format!(
                "capabilities/{}",
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let (sender, receiver) = oneshot::channel();
            self.capabilities_waiters.lock().await.insert(
                correlation_id.clone(),
                super::Probe {
                    from: address.to_string(),
                    tx: sender,
                },
            );
            let body = encode_task_frame(EncodeFrameInput {
                kind: TaskFrameKind::Capabilities,
                task_id: correlation_id.clone(),
                text: String::new(),
                ts: ::tinyplace::auth::timestamp(),
                correlation_id: Some(correlation_id.clone()),
                harness: None,
                provider: None,
                custom_harness: None,
                model: None,
                workflow: None,
                conversation: None,
            });
            if let Err(error) = self.relay.send(address, &body).await {
                self.capabilities_waiters
                    .lock()
                    .await
                    .remove(&correlation_id);
                return Err(RunError::Transport(error));
            }
            match tokio::time::timeout(self.ack_window, receiver).await {
                Ok(Ok(Ok(caps))) => return Ok(caps),
                Ok(Ok(Err(error))) => return Err(RunError::Worker(error)),
                Ok(Err(_)) => {
                    return Err(RunError::Transport(
                        "capabilities response channel closed".into(),
                    ));
                }
                Err(_) => {
                    self.capabilities_waiters
                        .lock()
                        .await
                        .remove(&correlation_id);
                    if attempt >= MAX_RESETS {
                        return Err(RunError::Timeout);
                    }
                    attempt += 1;
                    self.relay.reset_session(address).await;
                }
            }
        }
    }
}
