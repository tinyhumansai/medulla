use std::sync::Mutex;

use serde_json::json;

use crate::control_socket::grants::Grant;
use crate::control_socket::types::FleetWorker;
use crate::harness_hooks::HookReport;
use crate::hub::{RunError, TaskOutcome, TaskRequest};

use super::hook_report;

/// A [`FleetOps`](crate::control_socket::types::FleetOps) that only captures
/// what [`hook_report`] recorded, so these tests can inspect the summary that
/// actually reached the log rather than trusting what was sent on the wire.
#[derive(Default)]
struct RecordingFleet {
    recorded: Mutex<Option<HookReport>>,
}

#[async_trait::async_trait]
impl crate::control_socket::types::FleetOps for RecordingFleet {
    fn workers(&self) -> Option<Vec<FleetWorker>> {
        None
    }

    fn default_worker(&self) -> Option<String> {
        None
    }

    async fn dispatch(
        &self,
        _request: TaskRequest,
        _status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        unreachable!("hook_report never dispatches")
    }

    fn abort(&self, _abort_id: &str) -> bool {
        false
    }

    fn record_hook_event(&self, report: HookReport) {
        *self.recorded.lock().unwrap() = Some(report);
    }
}

/// A caller with a hook-only grant can reach `hook_report` directly — no
/// shim, no `commands::hook::sanitize` — the same way any subprocess that
/// inherits `MEDULLA_HOOK_GRANT` can. The handler itself must therefore strip
/// control bytes from `summary` before it is recorded, since the value is
/// later rendered straight into the operator's terminal via `Span::raw`
/// (chatgpt-codex-connector P2 on PR #192).
#[test]
fn a_summary_with_control_bytes_is_sanitized_before_it_is_recorded() {
    let fleet = std::sync::Arc::new(RecordingFleet::default());
    let ops: std::sync::Arc<dyn crate::control_socket::types::FleetOps> = fleet.clone();
    let grant = Grant::hook_only("s");

    let response = hook_report(
        &ops,
        &grant,
        &json!({
            "event": "Stop",
            "summary": "clear\x1b[2Jscreen\x07bell",
        }),
    )
    .expect("a hook-only grant may always report");
    assert_eq!(response, json!({ "recorded": true }));

    let recorded = fleet.recorded.lock().unwrap();
    let summary = &recorded
        .as_ref()
        .expect("hook_report always records")
        .summary;
    assert_eq!(summary, "clear[2Jscreenbell");
    assert!(
        !summary.chars().any(|c| c.is_control()),
        "a control byte reached the recorded summary: {summary:?}"
    );
}
