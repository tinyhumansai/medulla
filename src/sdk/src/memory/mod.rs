//! Memory service: a thin, medulla-owned wrapper over tinycortex's persona
//! memory layer (doc 06). It turns local coding-agent history into a durable,
//! prompt-ready persona pack and exposes a small offline query surface
//! (`status`/`search`/`directives`/`overview`) plus an LLM-backed ingest path.
//!
//! Vendor (`tinycortex`) types never cross this module's boundary: every result
//! is translated into a serde-friendly, medulla-owned type ([`MemoryStatus`],
//! [`MemoryHit`], [`IngestReport`]) so the UI and protocol layers stay decoupled
//! from the memory crate.

pub mod env;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use tinycortex::memory::config::{MemoryConfig, SecretString};
use tinycortex::memory::persona::state::FileStateStore;
use tinycortex::memory::persona::{
    compile, PersonaConfig, PersonaFacet, PersonaRetriever, Pipeline, RunMode,
};
use tinycortex::memory::providers::openrouter::{OpenRouterConfig, OpenRouterProvider};
use tinycortex::memory::score::extract::{ChatPrompt, ChatProvider};
use tinycortex::memory::tree::summarise::ConcatSummariser;

pub use env::MemorySettings;

impl IngestMode {
    fn into_run_mode(self) -> RunMode {
        match self {
            IngestMode::Backfill => RunMode::Backfill,
            IngestMode::Incremental => RunMode::Incremental,
        }
    }
}

/// Build the inference provider from resolved settings. An explicit
/// `OPENROUTER_API_KEY` wins (back-compat); otherwise the tinyhumans backend's
/// OpenAI-compatible surface is used with the resolved JWT and the
/// summarization model. With neither, the caller has no model available and
/// this errors — callers that can degrade (e.g. `medulla init`) should treat the
/// error as "run offline" rather than fatal.
///
/// Shared so every LLM-backed surface (memory ingest, workspace init) resolves
/// key, base URL, and model identically.
pub fn chat_provider(settings: &MemorySettings) -> Result<OpenRouterProvider> {
    let mut cfg = if let Some(key) = settings.openrouter_api_key.as_ref() {
        OpenRouterConfig {
            api_key: SecretString::new(key.clone()),
            run_cost_limit_usd: Some(settings.max_cost_usd),
            ..OpenRouterConfig::default()
        }
    } else if let Some(backend) = settings.backend.as_ref() {
        OpenRouterConfig {
            base_url: format!("{}/openai/v1", backend.base_url.trim_end_matches('/')),
            api_key: SecretString::new(backend.jwt.clone()),
            chat_model: env::DEFAULT_BACKEND_MODEL.to_string(),
            run_cost_limit_usd: Some(settings.max_cost_usd),
            ..OpenRouterConfig::default()
        }
    } else {
        return Err(anyhow!(
            "needs the backend (run `medulla login`) or OPENROUTER_API_KEY"
        ));
    };
    if let Some(model) = &settings.llm_model {
        cfg.chat_model = model.clone();
    }
    OpenRouterProvider::new(cfg)
}

#[async_trait::async_trait]
impl ChatProvider for NoopProvider {
    fn name(&self) -> &str {
        "noop"
    }
    async fn chat_for_json(&self, _prompt: &ChatPrompt) -> Result<String> {
        Err(anyhow!("offline compile does not call the chat provider"))
    }
}

impl MemoryService {
    /// Open the service against the resolved settings. Builds the tinycortex
    /// [`MemoryConfig`]; the retriever is loaded on first query.
    pub fn open(settings: MemorySettings) -> Result<Self> {
        let config = MemoryConfig::new(settings.workspace.clone());
        Ok(MemoryService {
            settings,
            config,
            retriever: Mutex::new(None),
        })
    }

    /// The resolved settings this service was opened with.
    pub fn settings(&self) -> &MemorySettings {
        &self.settings
    }

    /// Drop the cached retriever so the next query reloads it (after an ingest).
    pub fn reload(&self) {
        *self.retriever.lock().unwrap() = None;
    }

    /// Run `f` with the (lazily loaded) retriever, or `None` when the store is
    /// empty/absent.
    fn with_retriever<T>(&self, f: impl FnOnce(Option<&PersonaRetriever>) -> T) -> T {
        let mut guard = self.retriever.lock().unwrap();
        if guard.is_none() {
            *guard = Some(PersonaRetriever::load(&self.config).ok());
        }
        f(guard.as_ref().and_then(|inner| inner.as_ref()))
    }

    /// The compiled pack path (`<workspace>/persona/PERSONA.md`).
    pub fn pack_path(&self) -> String {
        compile::pack_path(&self.config).display().to_string()
    }

    /// The verbatim T0 directives (explicit standing rules).
    pub fn directives(&self) -> Vec<String> {
        compile::read_directives(&self.config)
    }

    /// A health snapshot of the memory layer.
    pub fn status(&self) -> MemoryStatus {
        let pack_path = compile::pack_path(&self.config);
        let (entry_count, facet_counts) = self.with_retriever(|r| match r {
            Some(retriever) => (
                retriever.len(),
                retriever
                    .facet_counts()
                    .into_iter()
                    .map(|(facet, n)| (facet.as_str().to_string(), n))
                    .collect(),
            ),
            None => (0, BTreeMap::new()),
        });
        MemoryStatus {
            enabled: self.settings.enabled,
            workspace: self.settings.workspace.display().to_string(),
            pack_exists: pack_path.exists(),
            pack_path: pack_path.display().to_string(),
            entry_count,
            directives_count: self.directives().len(),
            facet_counts,
        }
    }

    /// Rank the persona corpus against `query`. `facet` is a loose facet name
    /// (e.g. `stack`, `coding-style`); an unrecognized facet is ignored (no
    /// filter). Returns at most `k` hits.
    pub fn search(&self, query: &str, facet: Option<&str>, k: usize) -> Vec<MemoryHit> {
        let facet = facet.and_then(PersonaFacet::parse_loose);
        self.with_retriever(|r| match r {
            Some(retriever) => retriever
                .search(query, facet, k)
                .into_iter()
                .map(|hit| MemoryHit {
                    facet: hit.facet.as_str().to_string(),
                    tier: hit.tier.as_str().to_string(),
                    text: hit.text,
                    quote: hit.quote,
                    timestamp: hit.timestamp.to_rfc3339(),
                    score: hit.score,
                })
                .collect(),
            None => Vec::new(),
        })
    }

    /// A human-readable multi-line overview of the memory layer.
    pub fn overview(&self) -> String {
        let status = self.status();
        let mut out = String::new();
        out.push_str(&format!(
            "memory: {}\nworkspace: {}\npack: {} ({})\nobservations: {}\ndirectives: {}\n",
            if status.enabled {
                "enabled"
            } else {
                "disabled"
            },
            status.workspace,
            status.pack_path,
            if status.pack_exists {
                "present"
            } else {
                "absent"
            },
            status.entry_count,
            status.directives_count,
        ));
        if status.facet_counts.is_empty() {
            out.push_str("facets: (none)\n");
        } else {
            out.push_str("facets:\n");
            for (facet, n) in &status.facet_counts {
                out.push_str(&format!("  {facet}: {n}\n"));
            }
        }
        out
    }

    /// Build a [`PersonaConfig`] from the resolved settings and `home`.
    fn persona_config(&self, home: &Path) -> PersonaConfig {
        let mut persona = PersonaConfig::with_home(home, self.settings.identity.clone());
        if let Some(root) = &self.settings.claude_root {
            persona.claude_code_root = Some(root.clone());
        }
        if let Some(root) = &self.settings.codex_root {
            persona.codex_root = Some(root.clone());
        }
        if !self.settings.project_roots.is_empty() {
            persona.project_roots = self.settings.project_roots.clone();
        }
        if let Some(model) = &self.settings.llm_model {
            persona.chat_model = model.clone();
        }
        persona.run_budget.max_cost_usd = self.settings.max_cost_usd;
        persona
    }

    /// Build the inference provider (used as both `ChatProvider` and
    /// `Summariser`). An explicit `OPENROUTER_API_KEY` wins (back-compat);
    /// otherwise the tinyhumans backend's OpenAI-compatible surface is used
    /// with the resolved JWT and the summarization model. With neither, memory
    /// runs local-only (status/search/compile) and ingest errors clearly.
    fn provider(&self) -> Result<OpenRouterProvider> {
        chat_provider(&self.settings)
    }

    /// Run a live ingest pass (LLM-backed). Uses the backend summarizer (or an
    /// explicit OpenRouter key).
    pub async fn ingest(&self, mode: IngestMode) -> Result<IngestReport> {
        let home = home_dir();
        let persona = self.persona_config(&home);
        let provider = self.provider()?;
        let store = FileStateStore::open_in_workspace(&self.settings.workspace)?;
        let pipeline = Pipeline {
            config: &self.config,
            persona: &persona,
            provider: &provider,
            summariser: &provider,
            store: &store,
        };
        let report = pipeline.run(mode.into_run_mode()).await?;
        self.reload();
        Ok(from_run_report(report))
    }

    /// Recompile the pack from the persisted facet trees, no LLM calls.
    pub fn compile(&self) -> Result<IngestReport> {
        let home = home_dir();
        let persona = self.persona_config(&home);
        let provider = NoopProvider;
        let summariser = ConcatSummariser::new();
        let store = FileStateStore::open_in_workspace(&self.settings.workspace)?;
        let pipeline = Pipeline {
            config: &self.config,
            persona: &persona,
            provider: &provider,
            summariser: &summariser,
            store: &store,
        };
        let pack_path = pipeline.compile_only()?;
        self.reload();
        Ok(IngestReport {
            mode: "compile".to_string(),
            pack_path: Some(pack_path.display().to_string()),
            ..Default::default()
        })
    }
}

fn home_dir() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Translate a tinycortex `RunReport` into the medulla-owned [`IngestReport`].
fn from_run_report(report: tinycortex::memory::persona::RunReport) -> IngestReport {
    IngestReport {
        mode: report.mode,
        files_seen: report.files_seen,
        sessions_processed: report.sessions_processed,
        sessions_skipped: report.sessions_skipped,
        sessions_failed: report.sessions_failed,
        directives_folded: report.directives_folded,
        observations: report.observations,
        facet_counts: report.facet_counts.into_iter().collect(),
        budget_hit: report.budget_hit,
        pack_path: report.pack_path,
    }
}

#[cfg(test)]
mod tests;

mod types;
pub use types::IngestMode;
pub use types::IngestReport;
pub use types::MemoryHit;
pub use types::MemoryService;
pub use types::MemoryStatus;
use types::NoopProvider;
