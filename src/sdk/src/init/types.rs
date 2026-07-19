//! Data types for workspace initialisation: the instruction files read from a
//! directory, the profile fields an LLM drafts from them, and the outcome of a
//! write.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The instruction files `medulla init` distils a profile from. Every field is
/// optional — a directory with none of them still initialises (the draft just
/// has less to go on).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitSources {
    /// The directory these were read from.
    pub dir: PathBuf,
    /// Contents of `AGENTS.md`, when present.
    pub agents_md: Option<String>,
    /// Contents of `CLAUDE.md`, when present.
    pub claude_md: Option<String>,
    /// Contents of `README.md`, when present.
    pub readme_md: Option<String>,
}

impl InitSources {
    /// True when no instruction file was found at all.
    pub fn is_empty(&self) -> bool {
        self.agents_md.is_none() && self.claude_md.is_none() && self.readme_md.is_none()
    }

    /// The names of the files that were actually found, for operator output.
    pub fn found(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.agents_md.is_some() {
            names.push("AGENTS.md");
        }
        if self.claude_md.is_some() {
            names.push("CLAUDE.md");
        }
        if self.readme_md.is_some() {
            names.push("README.md");
        }
        names
    }
}

/// The profile fields drafted for a workspace. This is exactly the JSON shape
/// the drafting model is asked to return, so it deserialises straight from the
/// provider response; every field is defaulted so a partial answer still works.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftedProfile {
    /// The ~100-200 token orchestrator-facing summary (the Markdown body).
    #[serde(default)]
    pub summary: String,
    /// Advisory: harnesses to prefer here.
    #[serde(default)]
    pub harnesses: Vec<String>,
    /// Advisory: preferred reasoning-tier models.
    #[serde(default)]
    pub models_reasoning: Vec<String>,
    /// Advisory: freeform routing guidance, one hint per line.
    #[serde(default)]
    pub routing: Vec<String>,
}

impl DraftedProfile {
    /// The neutral draft used when no model is available: a stub the operator
    /// fills in by hand. Deterministic, so the offline path is testable.
    pub fn stub() -> Self {
        DraftedProfile {
            summary: STUB_SUMMARY.to_string(),
            harnesses: Vec::new(),
            models_reasoning: Vec::new(),
            routing: Vec::new(),
        }
    }

    /// True when this draft carries nothing an orchestrator could use.
    pub fn is_blank(&self) -> bool {
        self.summary.trim().is_empty()
            && self.harnesses.is_empty()
            && self.models_reasoning.is_empty()
            && self.routing.is_empty()
    }
}

/// Body written when there is no model to draft with — a prompt to the human.
pub const STUB_SUMMARY: &str =
    "TODO: describe this workspace in ~100-200 tokens — what it is, what \
the important entry points are, and any house rules that should shape how the \
orchestrator decomposes work here. Lead with a one-sentence identity for the \
repo; that first line is what shows up under the agent in `agent_list`.";

/// What `medulla init` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOutcome {
    /// Where the profile was written.
    pub path: PathBuf,
    /// The rendered `MEDULLA.md` contents.
    pub contents: String,
    /// True when an LLM drafted the body; false for the offline stub.
    pub drafted: bool,
    /// Instruction files the draft was based on.
    pub sources: Vec<&'static str>,
}
