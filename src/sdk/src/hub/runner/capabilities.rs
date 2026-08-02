//! Lightweight worker capability probes.

use tokio::sync::oneshot;

use crate::tinyplace::{encode_task_frame, AgentCapabilities, EncodeFrameInput, TaskFrameKind};

use super::types::CapabilityProbeGuard;
use super::{RunError, TaskRunner, CONTACT_POLL, CONTACT_WAIT, MAX_RESETS};

impl Drop for CapabilityProbeGuard {
    fn drop(&mut self) {
        if let Ok(mut waiters) = self.waiters.lock() {
            waiters.remove(&self.key);
        }
    }
}

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
        // A failed refresh must not leave support advertised by an older worker
        // that previously occupied this address.
        self.capabilities.lock().await.remove(address);
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
            self.capabilities_waiters.lock().unwrap().insert(
                correlation_id.clone(),
                super::Probe {
                    from: address.to_string(),
                    tx: sender,
                },
            );
            // The control-plane roster wraps this future in a shorter timeout
            // than the runner's retry window. Synchronous Drop cleanup makes
            // that external cancellation reap the registered sender too.
            let waiter_guard = CapabilityProbeGuard {
                waiters: self.capabilities_waiters.clone(),
                key: correlation_id.clone(),
            };
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
                tool_mode: None,
                workflow: None,
                workflow_fingerprint: None,
                workflow_inputs: Default::default(),
                conversation: None,
                fleet_depth: 0,
            });
            if let Err(error) = self.relay.send(address, &body).await {
                return Err(RunError::Transport(error));
            }
            match tokio::time::timeout(self.ack_window, receiver).await {
                Ok(Ok(Ok(caps))) => {
                    self.capabilities
                        .lock()
                        .await
                        .insert(address.to_string(), caps.clone());
                    return Ok(caps);
                }
                Ok(Ok(Err(error))) => return Err(RunError::Worker(error)),
                Ok(Err(_)) => {
                    return Err(RunError::Transport(
                        "capabilities response channel closed".into(),
                    ));
                }
                Err(_) => {
                    if attempt >= MAX_RESETS {
                        return Err(RunError::Timeout);
                    }
                    drop(waiter_guard);
                    attempt += 1;
                    self.relay.reset_session(address).await;
                }
            }
        }
    }

    /// Negotiate capabilities while making a backend abort effective before
    /// the task itself is registered and sent.
    pub async fn capabilities_for_dispatch(
        &self,
        address: &str,
        abort_id: &str,
    ) -> Result<AgentCapabilities, RunError> {
        let abort = std::sync::Arc::new(tokio::sync::Notify::new());
        self.aborts
            .lock()
            .expect("aborts lock")
            .insert(abort_id.to_string(), abort.clone());
        if !self.relay.contact_accepted(address).await {
            let _ = self.relay.request_contact(address).await;
            let deadline = std::time::Instant::now() + CONTACT_WAIT;
            while std::time::Instant::now() < deadline
                && !self.relay.contact_accepted(address).await
            {
                tokio::select! {
                    biased;
                    _ = abort.notified() => {
                        let mut aborts = self.aborts.lock().expect("aborts lock");
                        if aborts.get(abort_id).is_some_and(|current| {
                            std::sync::Arc::ptr_eq(current, &abort)
                        }) {
                            aborts.remove(abort_id);
                        }
                        return Err(RunError::Aborted);
                    },
                    _ = tokio::time::sleep(CONTACT_POLL) => {}
                }
            }
        }
        tokio::select! {
            biased;
            _ = abort.notified() => {
                let mut aborts = self.aborts.lock().expect("aborts lock");
                if aborts
                    .get(abort_id)
                    .is_some_and(|current| std::sync::Arc::ptr_eq(current, &abort))
                {
                    aborts.remove(abort_id);
                }
                Err(RunError::Aborted)
            }
            result = self.capabilities(address) => result,
        }
    }
}
