//! Data types for the `github` module.
#[allow(unused_imports)]
use super::*;
/// GitHub REST provider. The token is read from source configuration.
#[derive(Debug, Clone)]
pub struct GitHubProvider {
    pub(super) client: reqwest::Client,
}
#[derive(Debug, Deserialize)]
pub(super) struct GitHubRecord {
    pub(super) number: u64,
    pub(super) title: String,
    pub(super) body: Option<String>,
    pub(super) html_url: String,
    pub(super) state: String,
    pub(super) pull_request: Option<serde_json::Value>,
}
