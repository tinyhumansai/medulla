//! Data types for memory ingestion, search, and status reporting.
#[allow(unused_imports)]
use super::*;
/// Which ingest pass to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestMode {
    /// Walk everything, oldest-first.
    Backfill,
    /// Cursor-forward only: skip unchanged files/repos.
    Incremental,
}
/// A single retrieved persona observation, mirroring tinycortex's `PersonaHit`
/// with facet/tier flattened to strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryHit {
    /// Facet wire-string (e.g. `coding_style`).
    pub facet: String,
    /// Confidence tier (`t0`..`t3`).
    pub tier: String,
    /// Prescriptive observation text.
    pub text: String,
    /// Supporting quote, when present.
    pub quote: Option<String>,
    /// RFC3339 timestamp of the underlying evidence.
    pub timestamp: String,
    /// Final rank score (higher is better).
    pub score: f32,
}
/// A snapshot of the memory layer's health, for the Overview/CLI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryStatus {
    /// Whether the memory surface is enabled.
    pub enabled: bool,
    /// Workspace root.
    pub workspace: String,
    /// Whether a compiled `PERSONA.md` pack exists.
    pub pack_exists: bool,
    /// Compiled pack path.
    pub pack_path: String,
    /// Total indexed observations.
    pub entry_count: usize,
    /// Verbatim directive count.
    pub directives_count: usize,
    /// Per-facet observation counts (facet wire-string → count).
    pub facet_counts: BTreeMap<String, usize>,
}
/// Serde-friendly translation of tinycortex's `RunReport`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct IngestReport {
    /// The mode that ran (`backfill` / `incremental` / `compile`).
    pub mode: String,
    /// Transcript/instruction files discovered.
    pub files_seen: usize,
    /// Sessions/batches actually digested.
    pub sessions_processed: usize,
    /// Files skipped because their cursor was unchanged.
    pub sessions_skipped: usize,
    /// Sessions whose digest hit a hard provider failure.
    pub sessions_failed: usize,
    /// Instruction-file rules folded (verbatim T0).
    pub directives_folded: usize,
    /// Observations distilled.
    pub observations: usize,
    /// Per-facet observation counts.
    pub facet_counts: BTreeMap<String, usize>,
    /// True when a run budget stopped the run early.
    pub budget_hit: bool,
    /// Path of the compiled pack, if written.
    pub pack_path: Option<String>,
}
/// A no-op chat provider for offline compiles: the `compile_only` path never
/// calls it, but the [`Pipeline`] requires a `ChatProvider` to be bound.
pub(super) struct NoopProvider;
/// The medulla memory service. Cheap to hold; the BM25 retriever is loaded
/// lazily and cached, and can be dropped with [`MemoryService::reload`].
pub struct MemoryService {
    pub(super) settings: MemorySettings,
    pub(super) config: MemoryConfig,
    /// Cached retriever: `None` = not yet loaded; `Some(None)` = loaded but the
    /// store was absent/unreadable (treated as empty).
    pub(super) retriever: Mutex<Option<Option<PersonaRetriever>>>,
}
