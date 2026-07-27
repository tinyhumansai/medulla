//! Tests for lightweight worker capacity probes over the encrypted relay.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::hub::tests::dispatch::harness::{FakeWorker, Mode};
use crate::hub::{HubWorker, RunError, TaskRunner};
use crate::tinyplace::WorkerSystemInfo;

fn info() -> WorkerSystemInfo {
    WorkerSystemInfo {
        cpu_cores: 12,
        memory_total_bytes: Some(64 * 1024 * 1024 * 1024),
        memory_available_bytes: Some(40 * 1024 * 1024 * 1024),
        ip_address: "10.20.30.40".to_string(),
    }
}

#[test]
fn capacity_details_are_cached_only_for_the_peer_that_was_probed() {
    use super::super::handle::cache_system_info_if_current;

    let roster = Arc::new(Mutex::new(vec![HubWorker {
        id: "worker".to_string(),
        address: "address-new".to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
        workspace: None,
    }]));
    let cache = Mutex::new(HashMap::new());

    assert!(
        !cache_system_info_if_current(&roster, &cache, "worker", "address-old", info()),
        "a refresh started against the replaced peer must be discarded"
    );
    assert!(cache.lock().unwrap().is_empty());
    assert!(cache_system_info_if_current(
        &roster,
        &cache,
        "worker",
        "address-new",
        info()
    ));
    assert_eq!(cache.lock().unwrap().get("worker"), Some(&info()));
}

#[tokio::test]
async fn capacity_probe_returns_the_correlated_worker_details() {
    let worker = FakeWorker::new(Mode::SystemInfo(info()));
    let runner = TaskRunner::start_with_ack_window(
        worker.clone(),
        Duration::from_millis(5),
        Duration::from_millis(100),
    );

    let result = runner.system_info("worker").await.expect("capacity reply");

    assert_eq!(result, info());
    assert_eq!(worker.sent_kinds().await, vec!["system_info"]);
}

#[tokio::test]
async fn capacity_probe_repairs_a_desynchronized_session_and_retries() {
    let worker = FakeWorker::new(Mode::SystemInfoAfterReset(info()));
    let runner = TaskRunner::start_with_ack_window(
        worker.clone(),
        Duration::from_millis(5),
        Duration::from_millis(20),
    );

    let result = runner.system_info("worker").await.expect("retry succeeds");

    assert_eq!(result, info());
    assert_eq!(worker.resets.load(Ordering::Relaxed), 1);
    assert_eq!(
        worker.sent_kinds().await,
        vec!["system_info", "system_info"]
    );
}

#[tokio::test]
async fn capacity_probe_rejects_a_peer_that_has_not_accepted_contact() {
    let worker = FakeWorker::with(Mode::SystemInfo(info()), false, 1);
    let runner = TaskRunner::start(worker, Duration::from_millis(5));

    let error = runner
        .system_info("worker")
        .await
        .expect_err("not accepted");

    assert_eq!(
        error,
        RunError::Worker("worker has not accepted this hub contact yet".to_string())
    );
}

#[tokio::test]
async fn capacity_probe_surfaces_send_and_decode_failures() {
    let failing = FakeWorker::with(Mode::SystemInfo(info()), true, 0);
    let runner = TaskRunner::start(failing, Duration::from_millis(5));
    assert_eq!(
        runner.system_info("worker").await.expect_err("send fails"),
        RunError::Transport("send boom".to_string())
    );

    let malformed = FakeWorker::new(Mode::InvalidSystemInfo);
    let runner = TaskRunner::start(malformed, Duration::from_millis(5));
    let error = runner
        .system_info("worker")
        .await
        .expect_err("invalid response");
    assert!(
        matches!(error, RunError::Worker(message) if message.contains("invalid worker system info"))
    );
}

#[tokio::test]
async fn capacity_probe_times_out_and_reaps_its_waiter() {
    let worker = FakeWorker::new(Mode::Silent);
    let runner = TaskRunner::start_with_ack_window(
        worker,
        Duration::from_millis(5),
        Duration::from_millis(20),
    );

    assert_eq!(
        runner.system_info("worker").await.expect_err("times out"),
        RunError::Timeout
    );
}
