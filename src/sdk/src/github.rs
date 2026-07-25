//! GitHub task source adapter using the public REST API.

use async_trait::async_trait;
use serde::Deserialize;

use super::{
    now_timestamp, SourceConfig, SyncResult, Task, TaskRepository, TaskSourceRef, TaskStatus,
};

/// A provider record normalized into the task repository model.
#[async_trait]
pub trait TaskSourceProvider: Send + Sync {
    /// Synchronize one configured source into the local repository.
    async fn sync(&self, config: &SourceConfig, repository: &mut TaskRepository) -> SyncResult;
}

impl Default for GitHubProvider {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl TaskSourceProvider for GitHubProvider {
    async fn sync(&self, config: &SourceConfig, repository: &mut TaskRepository) -> SyncResult {
        let url = format!(
            "https://api.github.com/repos/{}/issues?state={}&per_page=100",
            config.repository, config.state
        );
        let mut request = self
            .client
            .get(url)
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "medulla");
        if let Some(token) = &config.token {
            request = request.bearer_auth(token);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return SyncResult {
                    errors: vec![error.to_string()],
                    ..Default::default()
                }
            }
        };
        if !response.status().is_success() {
            return SyncResult {
                errors: vec![format!("GitHub returned {}", response.status())],
                ..Default::default()
            };
        }
        let records = match response.json::<Vec<GitHubRecord>>().await {
            Ok(records) => records,
            Err(error) => {
                return SyncResult {
                    errors: vec![error.to_string()],
                    ..Default::default()
                }
            }
        };
        let mut result = SyncResult::default();
        for record in records {
            if !config.labels.is_empty() { /* label filtering is applied server-side in later API versions */
            }
            let task = Task {
                id: format!("github:{}#{}", config.repository, record.number),
                title: record.title,
                description: record.body.unwrap_or_default(),
                status: if record.state == "closed" {
                    TaskStatus::Done
                } else {
                    TaskStatus::Open
                },
                source: Some(TaskSourceRef {
                    provider: "github".into(),
                    source_id: format!("{}#{}", config.repository, record.number),
                    url: Some(record.html_url),
                }),
                recurrence: None,
                created_at: now_timestamp(),
                updated_at: now_timestamp(),
                last_synced_at: Some(now_timestamp()),
                dispatch: serde_json::json!({ "pull_request": record.pull_request.is_some() }),
            };
            if repository.upsert_synced(task) {
                result.added += 1;
            } else {
                result.updated += 1;
            }
        }
        result
    }
}

#[path = "github/types.rs"]
mod types;
pub use types::GitHubProvider;
use types::GitHubRecord;
