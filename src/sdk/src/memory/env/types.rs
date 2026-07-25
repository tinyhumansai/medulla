//! Data types for the `env` module.
#[allow(unused_imports)]
use super::*;
/// An inference target reachable through the tinyhumans backend's
/// OpenAI-compatible surface (`<baseUrl>/openai/v1`), authorized by the same
/// JWT the TUI resolves for the backend runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendInference {
    /// The backend base URL (no `/openai/v1` suffix).
    pub base_url: String,
    /// Bearer JWT.
    pub jwt: String,
}
/// The resolved, medulla-owned memory settings (no vendor types leak here).
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySettings {
    /// Whether the memory surface is active at all.
    pub enabled: bool,
    /// Workspace root for the SQLite chunk store, facet trees, and `persona/`.
    pub workspace: PathBuf,
    /// Identity line for the compiled pack header (email / name).
    pub identity: String,
    /// Claude Code transcript root override (`None` = tinycortex default).
    pub claude_root: Option<PathBuf>,
    /// Codex rollout root override (`None` = tinycortex default).
    pub codex_root: Option<PathBuf>,
    /// Project roots walked for instruction files + git history. Empty = default.
    pub project_roots: Vec<PathBuf>,
    /// Chat/digest model id override (`None` = tinycortex default).
    pub llm_model: Option<String>,
    /// Per-run provider spend ceiling, USD.
    pub max_cost_usd: f64,
    /// The OpenRouter API key, when present (explicit override for ingest).
    pub openrouter_api_key: Option<String>,
    /// Backend inference target: used for summarization when no OpenRouter key
    /// is set. `None` + no key → ingest is unavailable (local-only mode).
    pub backend: Option<BackendInference>,
}
