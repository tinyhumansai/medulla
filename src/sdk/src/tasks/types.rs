//! Data types for the `tasks` module.
#[allow(unused_imports)]
use super::*;
/// A task's local lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Task is available to begin.
    #[default]
    Open,
    /// Task is actively being worked.
    InProgress,
    /// Task completed successfully.
    Done,
    /// Task was intentionally abandoned.
    Cancelled,
}
/// A recurrence rule supported by the local repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recurrence {
    /// Repeat once per day.
    Daily,
    /// Repeat once per week.
    Weekly,
    /// Repeat once per month.
    Monthly,
    /// Repeat after the supplied number of days.
    EveryDays(u32),
}
/// A recurring task definition. Definitions produce instances but never dispatch them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurringTask {
    /// Rule used to calculate the next occurrence.
    pub recurrence: Recurrence,
    /// Timestamp or date of the next occurrence.
    pub next_at: String,
}
/// Stable identity and display information for an external source record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSourceRef {
    /// External provider wire name.
    pub provider: String,
    /// Provider-owned record identifier.
    pub source_id: String,
    /// Browser URL for the external record.
    pub url: Option<String>,
}
/// A local task, including fields intentionally reserved for future dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// Stable local task identifier.
    pub id: String,
    /// Short human-facing title.
    pub title: String,
    /// Full task instructions or context.
    pub description: String,
    /// Current local lifecycle state.
    pub status: TaskStatus,
    /// External source provenance, when synchronized.
    pub source: Option<TaskSourceRef>,
    /// Recurrence definition, when this task repeats.
    pub recurrence: Option<RecurringTask>,
    /// Creation timestamp.
    pub created_at: String,
    /// Most recent local update timestamp.
    pub updated_at: String,
    /// Most recent provider synchronization timestamp.
    pub last_synced_at: Option<String>,
    /// Reserved provider- or dispatcher-specific metadata.
    #[serde(default)]
    pub dispatch: serde_json::Value,
}
/// Configuration for one synchronized provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceConfig {
    /// Stable local source identifier.
    pub id: String,
    /// Provider adapter wire name.
    pub provider: String,
    /// Whether synchronization is enabled.
    pub enabled: bool,
    /// Provider repository or project locator.
    pub repository: String,
    /// Provider state filter, such as `open`.
    #[serde(default = "default_state")]
    pub state: String,
    /// Labels records must carry.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Optional provider-specific query filter.
    #[serde(default)]
    pub filter: Option<String>,
    /// Optional source credential; callers should prefer environment-backed auth.
    #[serde(default)]
    pub token: Option<String>,
}
/// Root JSON document stored under the Medulla home directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TaskDocument {
    /// Locally persisted tasks.
    pub tasks: Vec<Task>,
    /// Configured synchronization sources.
    pub sources: Vec<SourceConfig>,
}
/// Summary returned by a provider synchronization.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncResult {
    /// Records inserted locally.
    pub added: usize,
    /// Existing records changed from provider data.
    pub updated: usize,
    /// Existing records already up to date.
    pub unchanged: usize,
    /// Non-fatal record or provider errors.
    pub errors: Vec<String>,
}
/// Errors raised while loading or persisting the task document.
#[derive(Debug, Error)]
pub enum TaskRepositoryError {
    /// The repository's lifetime lock could not be acquired.
    #[error("could not lock task repository {path}: {source}")]
    Lock {
        /// Lock file that could not be opened or acquired.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The repository file could not be read.
    #[error("could not read task repository {path}: {source}")]
    Read {
        /// Repository path that failed.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// Repository JSON was malformed.
    #[error("malformed task repository {path}: {source}")]
    Parse {
        /// Repository path containing malformed JSON.
        path: PathBuf,
        /// JSON decoding error.
        source: serde_json::Error,
    },
    /// An atomic repository write failed.
    #[error("could not persist task repository {path}: {source}")]
    Write {
        /// Repository path that could not be persisted.
        path: PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The in-memory document could not be encoded as JSON.
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
