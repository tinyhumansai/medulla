//! Drafting a workspace profile from a directory's instruction files.
//!
//! `medulla init` reads `AGENTS.md` / `CLAUDE.md` / `README.md` — which are
//! written for coding agents working *inside* a repo, and are far too long for
//! an orchestrator's context — and asks a model to distil them into the short,
//! routing-oriented summary a `MEDULLA.md` carries.
//!
//! The model call goes through tinycortex's [`ChatProvider`], the same seam
//! `medulla memory ingest` uses, so key/base-URL/model resolution is shared (see
//! [`crate::memory::chat_provider`]).

use anyhow::{anyhow, Result};
use tinycortex::memory::score::extract::{ChatPrompt, ChatProvider};

use super::types::{DraftedProfile, InitSources};

/// Per-file cap on source text fed to the model. Instruction files can run to
/// tens of KB; the draft only needs the shape of the repo, and an unbounded
/// prompt is an unbounded bill.
const MAX_SOURCE_CHARS: usize = 8_000;

/// Sampling temperature — low, because this is a distillation, not creative
/// writing, and a stable draft is easier for an operator to review.
const TEMPERATURE: f32 = 0.2;

/// Output cap: the body is ~100-200 tokens plus a small amount of JSON scaffold.
const MAX_TOKENS: u32 = 900;

/// The extraction contract. The provider is asked for JSON so the response maps
/// straight onto [`DraftedProfile`].
const SYSTEM: &str = "You write MEDULLA.md workspace profiles for an AI orchestrator.

Given a repository's instruction files, produce a profile that tells an
orchestrator what the repository IS and how to route work over it.

Return ONLY a JSON object with these keys:
  \"summary\"          string  - 100-200 tokens of prose. Lead with a
                                one-sentence identity of the repo, then what it
                                does, its important entry points, and any house
                                rules that should shape how work is decomposed.
                                Write instructions for an orchestrator, not
                                marketing copy. No markdown headings.
  \"harnesses\"        array   - coding harnesses this repo suits, from
                                [\"claude-code\", \"opencode\", \"codex\"]. Omit
                                (empty array) if the sources give no signal.
  \"models_reasoning\" array   - preferred reasoning model ids, only if the
                                sources actually state a preference. Usually
                                empty.
  \"routing\"          array   - short routing hints, one per string, e.g.
                                \"UI work -> claude-code agents\". Empty if the
                                sources give no signal.

Never invent preferences the sources do not support: an empty array is the
correct answer when there is no signal. Output the JSON object and nothing
else.";

/// Truncate a source file to the per-file cap, marking the cut so the model
/// knows the text is partial.
fn clip(text: &str) -> String {
    if text.len() <= MAX_SOURCE_CHARS {
        return text.to_string();
    }
    let mut end = MAX_SOURCE_CHARS;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…(truncated)", &text[..end])
}

/// Assemble the user prompt from whichever instruction files were found.
pub fn build_user_prompt(sources: &InitSources) -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "Repository directory: {}",
        sources.dir.file_name().map_or_else(
            || sources.dir.display().to_string(),
            |name| name.to_string_lossy().to_string()
        )
    ));
    for (label, body) in [
        ("AGENTS.md", &sources.agents_md),
        ("CLAUDE.md", &sources.claude_md),
        ("README.md", &sources.readme_md),
    ] {
        if let Some(text) = body {
            parts.push(format!("\n--- {label} ---\n{}", clip(text)));
        }
    }
    parts.join("\n")
}

/// Pull the JSON object out of a provider response. Models occasionally wrap
/// JSON in prose or a ```json fence despite instructions, so the first `{` to
/// the last `}` is taken rather than trusting the whole body.
fn extract_json(response: &str) -> Result<&str> {
    let start = response
        .find('{')
        .ok_or_else(|| anyhow!("model response contained no JSON object"))?;
    let end = response
        .rfind('}')
        .ok_or_else(|| anyhow!("model response contained no JSON object"))?;
    if end <= start {
        return Err(anyhow!("model response contained no JSON object"));
    }
    Ok(&response[start..=end])
}

/// Parse a provider response into a drafted profile.
pub fn parse_draft(response: &str) -> Result<DraftedProfile> {
    let json = extract_json(response)?;
    let draft: DraftedProfile = serde_json::from_str(json)
        .map_err(|err| anyhow!("model response was not a valid profile: {err}"))?;
    if draft.is_blank() {
        return Err(anyhow!("model returned an empty profile"));
    }
    Ok(draft)
}

/// Ask the model to draft a profile from the given sources.
///
/// Errors when the provider fails or returns something unusable; callers that
/// can degrade should fall back to [`DraftedProfile::stub`] so `init` still
/// writes a hand-editable file offline.
pub async fn draft_profile(
    provider: &dyn ChatProvider,
    sources: &InitSources,
) -> Result<DraftedProfile> {
    let prompt = ChatPrompt {
        system: SYSTEM.to_string(),
        user: build_user_prompt(sources),
        temperature: TEMPERATURE,
        kind: "medulla-init-profile",
        max_tokens: Some(MAX_TOKENS),
    };
    let response = provider.chat_for_json(&prompt).await?;
    parse_draft(&response)
}
