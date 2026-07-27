//! Durable key/value state for stateful workflows.
//!
//! One JSON file per key, under a per-workflow namespace directory, so two
//! workflows can use the same key name without colliding. Keys are hashed into
//! their filename rather than used verbatim: a key is author-supplied and may
//! contain path separators, and a `StateStore` must never be a way to write
//! outside its own directory.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tinyflows::caps::StateStore;
use tinyflows::error::{EngineError, Result};

/// A [`StateStore`] over files beneath a namespace directory.
pub struct FileStateStore {
    /// The namespace's directory: `<state dir>/<namespace hash>`.
    dir: PathBuf,
}

impl FileStateStore {
    /// A store for `namespace` (conventionally `workflow:<id>`) under `root`.
    pub fn new(root: &Path, namespace: &str) -> Self {
        Self {
            dir: root.join(digest(namespace)),
        }
    }

    /// The file a key is stored in.
    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.json", digest(key)))
    }
}

/// A hex SHA-256 digest, used to turn an arbitrary author-supplied string into
/// one safe path component.
fn digest(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[async_trait]
impl StateStore for FileStateStore {
    async fn load(&self, key: &str) -> Result<Option<Value>> {
        let path = self.path(key);
        // `tokio::fs` rather than `std::fs`: these run on the runtime's worker
        // threads alongside every other node in the graph, and a blocking read
        // here stalls whatever else is scheduled there.
        match tokio::fs::read(&path).await {
            Ok(body) => serde_json::from_slice(&body)
                .map(Some)
                .map_err(|err| EngineError::Capability(format!("state: {key}: {err}"))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(EngineError::Capability(format!("state: {key}: {err}"))),
        }
    }

    async fn store(&self, key: &str, value: Value) -> Result<()> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|err| EngineError::Capability(format!("state: {key}: {err}")))?;
        let body = serde_json::to_vec(&value)
            .map_err(|err| EngineError::Capability(format!("state: {key}: {err}")))?;
        tokio::fs::write(self.path(key), body)
            .await
            .map_err(|err| EngineError::Capability(format!("state: {key}: {err}")))
    }
}
