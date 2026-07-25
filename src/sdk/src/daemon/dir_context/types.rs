//! Data types for the `dir_context` module.
#[allow(unused_imports)]
use super::*;
/// What the workspace's project files yield for one probe.
#[derive(Debug, Clone, Default)]
pub struct DirContext {
    /// Labeled, trimmed excerpts to append to the probe prompt; `None` when the
    /// workspace has none of the well-known files.
    pub prompt_block: Option<String>,
    /// Deterministic ≤[`MAX_SUMMARY_CHARS`] digest, the summary of last resort
    /// when the LLM probe fails or omits one.
    pub fallback_summary: Option<String>,
}
