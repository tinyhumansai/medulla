//! CoreClient transport-level branches, below the runtime fold loop: a response
//! missing its promised field, an oversize outbound frame rejected before the
//! socket, and an in-flight request drained as a transport error on disconnect.

use crate::helpers::*;
use crate::mock_core::{MockCore, MockCoreConfig};

// A response missing the promised field surfaces as a transport error.
#[tokio::test]
async fn missing_cycle_id_is_transport_error() {
    let (_dir, sock) = tmp_sock();
    let mut cfg = MockCoreConfig::default();
    cfg.responses.insert("cycle.submit".into(), json!({}));
    let _mock = MockCore::start_with(&sock, cfg).await;
    let (client, _rx) = CoreClient::connect(&sock).await.unwrap();

    let err = client
        .cycle_submit("th_test", "hi", None)
        .await
        .unwrap_err();
    assert!(matches!(err, CallError::Transport(_)), "{err}");
}

// An outbound frame over the 1 MiB cap is rejected before it hits the socket.
#[tokio::test]
async fn oversize_outbound_frame_rejected() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let (client, _rx) = CoreClient::connect(&sock).await.unwrap();

    let huge = "x".repeat(medulla::runtime::core_client::MAX_FRAME_BYTES + 1);
    let err = client
        .request("cycle.submit", json!({ "input": huge }))
        .await
        .unwrap_err();
    match err {
        CallError::Transport(m) => assert!(m.contains("1 MiB"), "{m}"),
        other => panic!("expected transport error, got {other}"),
    }
}

// A connection dropped while a request is in flight fails that request rather than
// hanging it.
#[tokio::test]
async fn request_drop_mid_flight_is_transport_error() {
    let (_dir, sock) = tmp_sock();
    let cfg = MockCoreConfig {
        close_on: Some("cycle.abort".into()),
        ..Default::default()
    };
    let _mock = MockCore::start_with(&sock, cfg).await;
    let (client, _rx) = CoreClient::connect(&sock).await.unwrap();

    // On disconnect the read loop drains every outstanding request with a synthetic
    // `transport.closed` RPC error rather than leaving it hung.
    let err = client.cycle_abort("c1").await.unwrap_err();
    assert!(
        matches!(err, CallError::Transport(_)) || err.rpc_code() == Some("transport.closed"),
        "{err}"
    );
}
