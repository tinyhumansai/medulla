//! A runnable wrapper around the in-test mock tiny.place **Signal server**
//! (`tests/support/mock_signal_server.rs`), so the coordination e2e suite can run
//! the exact same server as a standalone process the `medulla daemon` and the
//! owner helper talk to over loopback.
//!
//! The crypto is live: this server only stores and relays opaque pre-key bundles
//! and encrypted envelopes — real X3DH + double-ratchet runs inside the SDK on
//! both ends. See the support module for the full endpoint/state/fault spec.
//!
//! Usage: `cargo run --example mock_signal_server` — binds `127.0.0.1:$MOCK_SIGNAL_PORT`
//! (default an ephemeral port), prints `mock_signal_server listening on <url>` on
//! the first stdout line, then serves until killed.

#[path = "../tests/support/mock_signal_server.rs"]
mod mock_signal_server;

use mock_signal_server::MockSignalServer;

#[tokio::main]
async fn main() {
    let port = std::env::var("MOCK_SIGNAL_PORT").unwrap_or_else(|_| "0".to_string());
    let addr = format!("127.0.0.1:{port}");
    let server = MockSignalServer::start_on_addr(&addr).await;
    println!("mock_signal_server listening on {}", server.base_url);
    // Serve until the process is killed; the acceptor lives on the server handle.
    std::future::pending::<()>().await;
    drop(server);
}
