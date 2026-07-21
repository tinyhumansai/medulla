//! The non-interactive [`drive_once`](crate::runtime::headless::drive_once)
//! driver round-tripping one instruct against the in-crate `medulla-serve`
//! [`StubServer`]: it attaches the core runtime, submits an instruction, and the
//! streamed NDJSON transcript carries a `ready` line, the folded cycle events,
//! and a terminal `result` — the exact surface a docker e2e drives.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::runtime::headless::{drive_once, HeadlessOptions};
use crate::runtime::Runtime;

use super::super::client::CoreRuntime;
use super::super::stub_server::{StubConfig, StubServer};

/// Short timeouts so a regression fails fast instead of stalling the suite.
fn fast_opts() -> HeadlessOptions {
    HeadlessOptions {
        ready_timeout: Duration::from_secs(5),
        cycle_timeout: Duration::from_secs(5),
    }
}

/// Parse the NDJSON transcript into one JSON value per non-empty line.
fn lines(out: &[u8]) -> Vec<Value> {
    String::from_utf8(out.to_vec())
        .expect("transcript is utf-8")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("each line is a JSON object"))
        .collect()
}

#[tokio::test]
async fn headless_driver_round_trips_one_instruct_against_the_stub() {
    let server = StubServer::start(StubConfig::default());
    let runtime: Arc<dyn Runtime> = Arc::new(CoreRuntime::attach(server.path.clone()));

    let mut out: Vec<u8> = Vec::new();
    let summary = drive_once(runtime, "reconcile the world".into(), &mut out, fast_opts())
        .await
        .expect("the headless run should settle on the cycle result");

    let transcript = lines(&out);

    // First line announces the attached runtime and its serve identity.
    let ready = &transcript[0];
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["sessionId"], "agent");
    assert!(
        ready["runtime"].as_str().unwrap().contains("attached"),
        "ready line describes the attached runtime: {ready}"
    );

    // The last line is the terminal cycle result.
    let result = transcript.last().unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["passCount"], 0);
    assert_eq!(summary.pass_count, 0);

    // Between them, the streamed events carry the cycle framing the stub emits:
    // the optimistic user turn, a cycle_start, the task board row, and cycle_end.
    let event_kinds: Vec<String> = transcript
        .iter()
        .filter(|l| l["type"] == "event")
        .filter_map(|l| l["event"]["kind"].as_str().map(str::to_string))
        .collect();
    assert!(
        event_kinds.iter().any(|k| k == "cycle_start"),
        "streamed a cycle_start: {event_kinds:?}"
    );
    assert!(
        event_kinds.iter().any(|k| k == "cycle_end"),
        "streamed a cycle_end: {event_kinds:?}"
    );
    assert!(
        event_kinds.iter().any(|k| k == "user"),
        "streamed the optimistic user turn: {event_kinds:?}"
    );
    assert!(summary.events_streamed >= 3, "{}", summary.events_streamed);

    // The instruction actually reached serve.
    assert!(server.received_ops().contains(&"instruct".to_string()));
}

#[tokio::test]
async fn headless_driver_surfaces_a_rejected_instruct() {
    // The stub answers `instruct` with ok:false; the driver must propagate that
    // as an error rather than wait out the cycle timeout.
    let server = StubServer::start(StubConfig {
        instruct_fail: true,
        ..StubConfig::default()
    });
    let runtime: Arc<dyn Runtime> = Arc::new(CoreRuntime::attach(server.path.clone()));

    let mut out: Vec<u8> = Vec::new();
    let err = drive_once(runtime, "do it".into(), &mut out, fast_opts())
        .await
        .expect_err("a rejected instruct must fail the run");
    assert!(err.to_string().contains("instruct failed"), "{err}");

    // The `ready` line was still emitted before the submit failed.
    let transcript = lines(&out);
    assert_eq!(transcript[0]["type"], "ready");
    assert!(transcript.iter().all(|l| l["type"] != "result"));
}

#[tokio::test]
async fn headless_driver_times_out_when_the_cycle_never_ends() {
    // The stub acks the instruct and streams a `cycle_start` but never a
    // `cycle_end`, so the driver waits out `cycle_timeout` and reports it rather
    // than blocking forever. The `ready` line and the partial event still stream.
    let server = StubServer::start(StubConfig {
        instruct_events: vec![
            json!({"kind":"cycle_start","instructionId":"inst-agent-0","cycleId":"cyc:agent:0"}),
        ],
        ..StubConfig::default()
    });
    let runtime: Arc<dyn Runtime> = Arc::new(CoreRuntime::attach(server.path.clone()));

    let mut out: Vec<u8> = Vec::new();
    let err = drive_once(
        runtime,
        "spin".into(),
        &mut out,
        HeadlessOptions {
            ready_timeout: Duration::from_secs(5),
            cycle_timeout: Duration::from_millis(300),
        },
    )
    .await
    .expect_err("a cycle that never ends must time out");
    assert!(err.to_string().contains("cycle to finish"), "{err}");

    // The transcript still opened with `ready` and streamed the partial cycle,
    // but never reached a `result`.
    let transcript = lines(&out);
    assert_eq!(transcript[0]["type"], "ready");
    assert!(transcript.iter().all(|l| l["type"] != "result"));
}

#[tokio::test]
async fn headless_driver_times_out_waiting_to_attach() {
    // Point at a socket path with no listener: the runtime can never reach
    // `Live`, so the attach wait expires and nothing is streamed.
    let dead = std::env::temp_dir().join(format!(
        "mdl-headless-dead-{}-{:?}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&dead);
    let runtime: Arc<dyn Runtime> = Arc::new(CoreRuntime::attach(dead));

    let mut out: Vec<u8> = Vec::new();
    let err = drive_once(
        runtime,
        "hi".into(),
        &mut out,
        HeadlessOptions {
            ready_timeout: Duration::from_millis(150),
            cycle_timeout: Duration::from_secs(5),
        },
    )
    .await
    .expect_err("an attach that never completes must time out");
    assert!(err.to_string().contains("waiting for the runtime"), "{err}");
    assert!(out.is_empty());
}

#[tokio::test]
async fn headless_driver_reports_an_unavailable_runtime() {
    // A protocol mismatch latches the runtime unavailable; the driver reports it
    // instead of hanging on the never-arriving cycle.
    let server = StubServer::start(StubConfig {
        protocol: 2,
        ..StubConfig::default()
    });
    let runtime: Arc<dyn Runtime> = Arc::new(CoreRuntime::attach(server.path.clone()));

    let mut out: Vec<u8> = Vec::new();
    let err = drive_once(runtime, "hi".into(), &mut out, fast_opts())
        .await
        .expect_err("an unavailable runtime must fail the run");
    assert!(err.to_string().contains("unavailable"), "{err}");
    // Nothing was streamed: the attach never reached Live.
    assert!(out.is_empty());
}
