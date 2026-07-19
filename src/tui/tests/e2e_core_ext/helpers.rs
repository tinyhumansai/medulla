//! Shared fixtures for the extended core-runtime e2e suite: the per-wait read
//! timeout, a `connect` helper that handshakes a [`CoreRuntime`] over the mock
//! core, and a throwaway Unix-socket path. Re-exports the serde/medulla types
//! the grouped test modules need so they can rely on `use crate::helpers::*;`.

pub use std::path::Path;
pub use std::time::Duration;

pub use serde_json::json;

pub use medulla::runtime::core::CoreRuntime;
pub use medulla::runtime::core_client::{CallError, CoreClient};
pub use medulla::runtime::{Runtime, StreamState, WorkerOp};
pub use medulla_tui::ui::agents::derive_agent_lanes;

/// Shared per-wait timeout for the extended core suite.
pub const T: Duration = Duration::from_secs(5);

/// Connect a [`CoreRuntime`] to the mock core listening on `sock`.
pub async fn connect(sock: &Path) -> CoreRuntime {
    let (client, rx) = CoreClient::connect(sock).await.unwrap();
    CoreRuntime::connect(client, rx, "test", None)
        .await
        .unwrap()
}

/// A throwaway temp dir and the Unix-socket path inside it for one mock core.
pub fn tmp_sock() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("core.sock");
    (dir, sock)
}
