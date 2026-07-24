//! Answering a routing hub's lightweight worker system-information request.

use crate::tinyplace::{capture_system_info, TaskFrame, TaskFrameKind};

use super::super::types::DaemonRuntime;

impl DaemonRuntime {
    /// Capture current capacity details and return them without invoking a model.
    pub(super) async fn handle_system_info(&self, from: String, frame: TaskFrame) {
        // macOS probes launch `sysctl` and `vm_stat`, while Linux reads procfs;
        // keep those blocking operations off the async executor.
        let info = tokio::task::spawn_blocking(capture_system_info)
            .await
            .unwrap_or_default();
        let text = serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string());
        self.reply(
            &from,
            TaskFrameKind::SystemInfoResult,
            &frame.task_id,
            &text,
            frame.correlation_id.as_deref(),
            Some(self.inner.config.default_provider),
        )
        .await;
    }
}
