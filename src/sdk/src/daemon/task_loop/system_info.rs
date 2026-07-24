//! Answering a routing hub's lightweight worker system-information request.

use crate::tinyplace::{capture_system_info, TaskFrame, TaskFrameKind};

use super::super::types::DaemonRuntime;

impl DaemonRuntime {
    /// Capture current capacity details and return them without invoking a model.
    pub(super) async fn handle_system_info(&self, from: String, frame: TaskFrame) {
        let text =
            serde_json::to_string(&capture_system_info()).unwrap_or_else(|_| "{}".to_string());
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
