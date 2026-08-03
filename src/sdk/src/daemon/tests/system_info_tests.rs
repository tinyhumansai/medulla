//! Tests for daemon-side lightweight system-information responses.

use crate::daemon::providers::RunTaskFn;
use crate::daemon::DaemonRuntime;
use crate::protocol::{TaskFrameKind, WorkerSystemInfo};

use super::{base_config, decoded_frames, recording_send, task_frame};

#[tokio::test]
async fn system_info_request_replies_without_running_a_harness() {
    let run_task: RunTaskFn = std::sync::Arc::new(|_| {
        Box::pin(async { panic!("system information must not invoke a provider") })
    });
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);
    let mut request = task_frame("capacity-1", "", Some("probe-7"));
    request.kind = TaskFrameKind::SystemInfo;

    runtime.handle_message("hub".to_string(), String::new(), Some(request));
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let response = frames.first().expect("system information response");
    assert_eq!(response.kind, TaskFrameKind::SystemInfoResult);
    assert_eq!(response.task_id, "capacity-1");
    assert_eq!(response.correlation_id.as_deref(), Some("probe-7"));
    let info: WorkerSystemInfo = serde_json::from_str(&response.text).expect("valid details");
    assert!(info.cpu_cores > 0);
    assert!(info.memory_total_bytes >= info.memory_available_bytes);
}
