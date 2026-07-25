//! Data types for the `tasks` module.
#[allow(unused_imports)]
use super::*;
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
    #[error("could not lock task repository {path}: {source}")]
    Lock {
        /// Lock file that could not be opened or acquired.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
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
/// JSON-backed repository with exclusive lifetime locking and atomic saves.
///
/// Public constructors acquire a sibling lock file before reading and retain it
/// until every clone is dropped, serializing the complete read-mutate-save
/// lifecycle across threads and processes.
#[derive(Debug, Clone)]
pub struct TaskRepository {
    pub(super) path: PathBuf,
    pub(super) document: TaskDocument,
    pub(super) _lock: Option<Arc<File>>,
}
