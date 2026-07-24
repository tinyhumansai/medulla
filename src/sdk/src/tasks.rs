//! Durable local tasks and provider configuration.
//!
//! The repository is deliberately independent from the runtime/orchestrator:
//! tasks are records an operator can edit and synchronize, not work to dispatch.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[path = "github.rs"]
pub mod github;

/// A task's local lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    #[default]
    Open,
    InProgress,
    Done,
    Cancelled,
}

/// A recurrence rule supported by the local repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recurrence {
    Daily,
    Weekly,
    Monthly,
    EveryDays(u32),
}

/// A recurring task definition. Definitions produce instances but never dispatch them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurringTask {
    pub recurrence: Recurrence,
    pub next_at: String,
}

/// Stable identity and display information for an external source record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSourceRef {
    pub provider: String,
    pub source_id: String,
    pub url: Option<String>,
}

/// A local task, including fields intentionally reserved for future dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub source: Option<TaskSourceRef>,
    pub recurrence: Option<RecurringTask>,
    pub created_at: String,
    pub updated_at: String,
    pub last_synced_at: Option<String>,
    #[serde(default)]
    pub dispatch: serde_json::Value,
}

/// Configuration for one synchronized provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceConfig {
    pub id: String,
    pub provider: String,
    pub enabled: bool,
    pub repository: String,
    #[serde(default = "default_state")]
    pub state: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}
fn default_state() -> String {
    "open".into()
}

/// Root JSON document stored under the Medulla home directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TaskDocument {
    pub tasks: Vec<Task>,
    pub sources: Vec<SourceConfig>,
}

/// Summary returned by a provider synchronization.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncResult {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub errors: Vec<String>,
}

/// Errors raised while loading or persisting the task document.
#[derive(Debug, Error)]
pub enum TaskRepositoryError {
    #[error("could not read task repository {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("malformed task repository {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("could not persist task repository {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not serialize task repository: {0}")]
    Serialize(serde_json::Error),
}

/// JSON-backed repository with atomic replacement on save.
#[derive(Debug, Clone)]
pub struct TaskRepository {
    path: PathBuf,
    document: TaskDocument,
}

impl TaskRepository {
    /// Open an existing document, or return an empty repository when absent.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, TaskRepositoryError> {
        let path = path.into();
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
        Ok(Self { path, document })
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
