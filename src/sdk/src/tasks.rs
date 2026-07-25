//! Durable local tasks and provider configuration.
//!
//! The repository is deliberately independent from the runtime/orchestrator:
//! tasks are records an operator can edit and synchronize, not work to dispatch.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[path = "github.rs"]
pub mod github;

fn default_state() -> String {
    "open".into()
}

impl TaskRepository {
    /// Open an existing document, or return an empty repository when absent.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, TaskRepositoryError> {
        let path = path.into();
        let lock_path = path.with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| TaskRepositoryError::Lock {
                path: lock_path.clone(),
                source,
            })?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| TaskRepositoryError::Lock {
                path: lock_path.clone(),
                source,
            })?;
        lock.lock_exclusive()
            .map_err(|source| TaskRepositoryError::Lock {
                path: lock_path,
                source,
            })?;
        let document = match std::fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).map_err(|source| TaskRepositoryError::Parse {
                    path: path.clone(),
                    source,
                })?
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => TaskDocument::default(),
            Err(source) => return Err(TaskRepositoryError::Read { path, source }),
        };
        Ok(Self {
            path,
            document,
            _lock: Some(Arc::new(lock)),
        })
    }
    /// Open `<home>/tasks.json`.
    pub fn in_home(home: impl AsRef<Path>) -> Result<Self, TaskRepositoryError> {
        Self::open(home.as_ref().join("tasks.json"))
    }
    /// Access the current document.
    pub fn document(&self) -> &TaskDocument {
        &self.document
    }
    /// Mutate the document in memory.
    pub fn document_mut(&mut self) -> &mut TaskDocument {
        &mut self.document
    }
    /// Atomically persist the current document.
    pub fn save(&self) -> Result<(), TaskRepositoryError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| TaskRepositoryError::Write {
                path: self.path.clone(),
                source,
            })?;
        }
        let bytes =
            serde_json::to_vec_pretty(&self.document).map_err(TaskRepositoryError::Serialize)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(|source| TaskRepositoryError::Write {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|source| TaskRepositoryError::Write {
            path: self.path.clone(),
            source,
        })
    }
    /// Insert or merge a provider record, preserving local title/description/status edits.
    pub fn upsert_synced(&mut self, incoming: Task) -> bool {
        if let Some(existing) = self
            .document
            .tasks
            .iter_mut()
            .find(|t| t.source == incoming.source && t.source.is_some())
        {
            existing.last_synced_at = incoming.last_synced_at;
            if existing.title.is_empty() {
                existing.title = incoming.title;
            }
            if existing.description.is_empty() {
                existing.description = incoming.description;
            }
            existing.updated_at = incoming.updated_at;
            false
        } else {
            self.document.tasks.push(incoming);
            true
        }
    }
    /// Generate a concrete instance when a recurring definition is due.
    pub fn recurring_instance(task: &Task) -> Option<Task> {
        let recurrence = task.recurrence.as_ref()?;
        let mut instance = task.clone();
        instance.id = Uuid::new_v4().to_string();
        instance.recurrence = None;
        instance.status = TaskStatus::Open;
        instance.created_at = recurrence.next_at.clone();
        Some(instance)
    }
}

/// Current UTC timestamp represented without adding a date-time dependency.
pub fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
#[path = "tasks/tests.rs"]
mod tests;

mod types;
pub use types::Recurrence;
pub use types::RecurringTask;
pub use types::SourceConfig;
pub use types::SyncResult;
pub use types::Task;
pub use types::TaskDocument;
pub use types::TaskRepository;
pub use types::TaskRepositoryError;
pub use types::TaskSourceRef;
pub use types::TaskStatus;
