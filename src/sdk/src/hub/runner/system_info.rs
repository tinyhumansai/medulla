//! Lightweight worker system-information requests.

use tokio::sync::oneshot;

use crate::tinyplace::{encode_task_frame, EncodeFrameInput, TaskFrameKind, WorkerSystemInfo};

use super::{RunError, TaskRunner, MAX_RESETS};

impl TaskRunner {
    /// Request current CPU, memory, and primary-IP details from one worker.
    ///
    /// This uses the normal encrypted peer channel but never starts a harness or
    /// invokes a model. The request is bounded by the runner's acknowledgement
    /// window so a stale worker cannot leave a Routing refresh pending forever.
    pub async fn system_info(&self, address: &str) -> Result<WorkerSystemInfo, RunError> {
        if !self.relay.contact_accepted(address).await {
            let _ = self.relay.request_contact(address).await;
            return Err(RunError::Worker(
                "worker has not accepted this hub contact yet".into(),
            ));
        }
        let mut attempt = 0;
        loop {
            let correlation_id = format!(
                "system-info/{}",
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let (sender, receiver) = oneshot::channel();
            self.system_info_waiters.lock().await.insert(
                correlation_id.clone(),
                super::Probe {
                    from: address.to_string(),
                    tx: sender,
                },
            );
            let body = encode_task_frame(EncodeFrameInput {
                kind: TaskFrameKind::SystemInfo,
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
                workflow_inputs: Default::default(),
                conversation: None,
                fleet_depth: 0,
            });
            if let Err(error) = self.relay.send(address, &body).await {
                self.system_info_waiters
                    .lock()
                    .await
                    .remove(&correlation_id);
                return Err(RunError::Transport(error));
            }
            match tokio::time::timeout(self.ack_window, receiver).await {
                Ok(Ok(Ok(info))) => return Ok(info),
                Ok(Ok(Err(error))) => return Err(RunError::Worker(error)),
                Ok(Err(_)) => {
                    return Err(RunError::Transport(
                        "system-info response channel closed".into(),
                    ));
                }
                Err(_) => {
                    self.system_info_waiters
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
