//! A real listener driven by a real client over a real socket.
//!
//! The unit tests above call the handler directly, which cannot catch a framing
//! or correlation mismatch between the two halves — both sides could be
//! individually correct and still fail to talk. This is the test that would.

use std::sync::Arc;

use serde_json::json;

use super::super::client::ControlClient;
use super::super::grants::{Grant, GrantRegistry};
use super::super::server::ControlServer;
use super::super::types::{ControlError, ErrorKind, FleetOps};
use super::{FakeFleet, FakeOutcome};

/// A bound server over `fleet`, plus a token for a grant that may dispatch.
async fn serve(fleet: FakeFleet) -> (tempfile::TempDir, ControlServer, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control.sock");
    let grants = GrantRegistry::new();
    let token = grants.mint(Grant::new("session", 0, 2));
    let ops: Arc<dyn FleetOps> = Arc::new(fleet);
    let server = ControlServer::bind(&path, ops, grants, false)
        .await
        .expect("the socket should bind");
    (dir, server, token)
}

#[tokio::test]
async fn a_dispatch_round_trips_from_handle_to_reply() {
    let (_dir, server, token) = serve(FakeFleet::new()).await;

    let mut client = ControlClient::connect(server.path(), &token).await.unwrap();
    assert!(client.hello().hub_ready);
    assert!(client.hello().may_dispatch());

    let dispatched = client
        .call("task.dispatch", json!({ "instruction": "do it" }))
        .await
        .unwrap();
    let task_id = dispatched["taskId"].as_str().unwrap().to_string();

    let settled = client
        .call("task.get", json!({ "taskId": task_id, "waitSeconds": 5 }))
        .await
        .unwrap();

    assert_eq!(settled["status"], json!("done"));
    assert_eq!(settled["reply"], json!("done"));
}

#[tokio::test]
async fn several_calls_on_one_connection_stay_correlated() {
    // Ids increase per call and the client matches replies by id; a server that
    // answered out of order, or a client that took the first line it saw, would
    // hand a caller somebody else's answer.
    let (_dir, server, token) = serve(FakeFleet::new()).await;
    let mut client = ControlClient::connect(server.path(), &token).await.unwrap();

    let workers = client.call("worker.list", json!({})).await.unwrap();
    assert_eq!(workers["workers"][0]["id"], json!("alpha"));

    let first = client
        .call("task.dispatch", json!({ "instruction": "first" }))
        .await
        .unwrap();
    let second = client
        .call("task.dispatch", json!({ "instruction": "second" }))
        .await
        .unwrap();
    assert_ne!(first["taskId"], second["taskId"]);

    let listed = client.call("task.list", json!({})).await.unwrap();
    assert_eq!(listed["tasks"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_hanging_task_can_be_aborted_over_the_wire() {
    let (_dir, server, token) = serve(FakeFleet::new().with_outcome(FakeOutcome::Hang)).await;
    let mut client = ControlClient::connect(server.path(), &token).await.unwrap();

    let dispatched = client
        .call("task.dispatch", json!({ "instruction": "forever" }))
        .await
        .unwrap();
    let task_id = dispatched["taskId"].as_str().unwrap().to_string();

    let polled = client
        .call(
            "task.get",
            json!({ "taskId": task_id.clone(), "waitSeconds": 0 }),
        )
        .await
        .unwrap();
    assert_eq!(polled["status"], json!("running"));

    client
        .call("task.abort", json!({ "taskId": task_id.clone() }))
        .await
        .unwrap();

    let settled = client
        .call("task.get", json!({ "taskId": task_id, "waitSeconds": 5 }))
        .await
        .unwrap();
    assert_eq!(settled["status"], json!("aborted"));
}

#[tokio::test]
async fn connecting_where_nothing_listens_is_no_instance_not_an_error() {
    // The ordinary case for a harness whose spawn carried no grant. It must be a
    // value the shim can turn into a readable refusal, never a reason to die.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent.sock");

    let result = ControlClient::connect(&path, "irrelevant").await;

    assert!(matches!(result, Err(ControlError::NoInstance)));
}

#[tokio::test]
async fn a_bad_grant_is_refused_at_the_handshake() {
    let (_dir, server, _token) = serve(FakeFleet::new()).await;

    let result = ControlClient::connect(server.path(), "not-a-grant").await;

    match result {
        Ok(_) => panic!("a grant that was never minted must not connect"),
        Err(ControlError::Refused(failure)) => {
            assert_eq!(failure.kind, ErrorKind::Unauthenticated);
        }
        Err(other) => panic!("expected a refusal, got {other}"),
    }
}

#[tokio::test]
async fn a_revoked_grant_stops_working_for_new_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control.sock");
    let grants = GrantRegistry::new();
    let token = grants.mint(Grant::new("ending", 0, 2));
    let ops: Arc<dyn FleetOps> = Arc::new(FakeFleet::new());
    let server = ControlServer::bind(&path, ops, grants.clone(), false)
        .await
        .unwrap();

    assert!(ControlClient::connect(server.path(), &token).await.is_ok());
    grants.revoke("ending");

    assert!(matches!(
        ControlClient::connect(server.path(), &token).await,
        Err(ControlError::Refused(_))
    ));
}

#[tokio::test]
async fn a_revoked_grant_stops_working_on_an_open_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control.sock");
    let grants = GrantRegistry::new();
    let token = grants.mint(Grant::new("ending", 0, 2));
    let ops: Arc<dyn FleetOps> = Arc::new(FakeFleet::new());
    let _server = ControlServer::bind(&path, ops, grants.clone(), false)
        .await
        .unwrap();
    let mut client = ControlClient::connect(&path, &token).await.unwrap();

    grants.revoke("ending");

    assert!(matches!(
        client.call("worker.list", json!({})).await,
        Err(ControlError::Refused(failure)) if failure.kind == ErrorKind::Unauthenticated
    ));
}

#[tokio::test]
async fn a_dropped_server_surfaces_as_disconnected_and_cleans_up() {
    let (_dir, server, token) = serve(FakeFleet::new()).await;
    let path = server.path().to_path_buf();
    let mut client = ControlClient::connect(server.path(), &token).await.unwrap();

    drop(server);
    // The accept task is aborted asynchronously; give it a moment to unwind
    // before asserting the socket file is gone.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(!path.exists(), "the socket file should be unlinked on drop");
    assert!(matches!(
        client.call("worker.list", json!({})).await,
        Err(ControlError::Disconnected(_))
    ));
}

#[tokio::test]
async fn a_second_server_will_not_steal_a_live_address() {
    let (_dir, server, _token) = serve(FakeFleet::new()).await;
    let ops: Arc<dyn FleetOps> = Arc::new(FakeFleet::new());

    let second = ControlServer::bind(server.path(), ops, GrantRegistry::new(), false).await;

    assert!(
        second.is_err(),
        "the first binder must keep the address; two fleets on one socket \
         makes 'where did my task go' unanswerable"
    );
}

#[tokio::test]
async fn a_malformed_frame_is_answered_rather_than_dropped() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (_dir, server, _token) = serve(FakeFleet::new()).await;
    let stream = tokio::net::UnixStream::connect(server.path())
        .await
        .unwrap();
    let (read_half, mut write) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    write.write_all(b"{not json}\n").await.unwrap();
    write.flush().await.unwrap();

    let reply: serde_json::Value =
        serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(reply["ok"], json!(false));
    assert_eq!(reply["error"]["kind"], json!("badRequest"));
}
