//! Unit tests for workspace initialisation. Every test is offline and
//! deterministic: the drafting path is exercised through a stub `ChatProvider`
//! rather than a live model.

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use tinycortex::memory::score::extract::{ChatPrompt, ChatProvider};

use super::*;
use crate::init::types::STUB_SUMMARY;

/// A unique scratch directory per test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("medulla-init-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A provider that returns a canned response (or an error).
struct StubProvider(Result<String, String>);

#[async_trait::async_trait]
impl ChatProvider for StubProvider {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat_for_json(&self, _prompt: &ChatPrompt) -> Result<String> {
        self.0.clone().map_err(|err| anyhow!(err))
    }
}

const GOOD_JSON: &str = r#"{
  "summary": "Payments service. Owns billing, invoices, and the Stripe integration.",
  "harnesses": ["claude-code", "opencode"],
  "models_reasoning": [],
  "routing": ["Billing changes -> the payments agent."]
}"#;

// ── sources ─────────────────────────────────────────────────────────────────

#[test]
fn read_sources_collects_present_files_and_skips_missing() {
    let dir = scratch("sources");
    fs::write(dir.join("AGENTS.md"), "agents body").unwrap();
    fs::write(dir.join("README.md"), "readme body").unwrap();

    let sources = read_sources(&dir);
    assert_eq!(sources.agents_md.as_deref(), Some("agents body"));
    assert_eq!(sources.readme_md.as_deref(), Some("readme body"));
    assert_eq!(sources.claude_md, None);
    assert!(!sources.is_empty());
    assert_eq!(sources.found(), vec!["AGENTS.md", "README.md"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn read_sources_treats_blank_files_as_absent() {
    let dir = scratch("blank");
    fs::write(dir.join("AGENTS.md"), "   \n\n").unwrap();
    let sources = read_sources(&dir);
    assert_eq!(sources.agents_md, None);
    assert!(sources.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn read_sources_on_missing_dir_is_empty_not_an_error() {
    let sources = read_sources(&PathBuf::from("/no/such/dir"));
    assert!(sources.is_empty());
    assert!(sources.found().is_empty());
}

// ── draft parsing ───────────────────────────────────────────────────────────

#[test]
fn parse_draft_reads_the_documented_json() {
    let draft = parse_draft(GOOD_JSON).expect("parses");
    assert_eq!(draft.harnesses, vec!["claude-code", "opencode"]);
    assert!(draft.summary.starts_with("Payments service."));
    assert_eq!(draft.routing.len(), 1);
    assert!(draft.models_reasoning.is_empty());
}

#[test]
fn parse_draft_tolerates_a_fenced_or_prose_wrapped_object() {
    let wrapped = format!("Here you go:\n```json\n{GOOD_JSON}\n```\n");
    let draft = parse_draft(&wrapped).expect("parses");
    assert!(draft.summary.starts_with("Payments service."));
}

#[test]
fn parse_draft_rejects_non_json_and_empty_profiles() {
    assert!(parse_draft("no json here").is_err());
    assert!(parse_draft("{}").is_err());
    assert!(parse_draft(r#"{"summary": "  "}"#).is_err());
}

#[test]
fn build_user_prompt_includes_each_found_file() {
    let dir = scratch("prompt");
    fs::write(dir.join("AGENTS.md"), "agent rules here").unwrap();
    fs::write(dir.join("README.md"), "readme text here").unwrap();
    let prompt = build_user_prompt(&read_sources(&dir));
    assert!(prompt.contains("AGENTS.md"));
    assert!(prompt.contains("agent rules here"));
    assert!(prompt.contains("readme text here"));
    assert!(!prompt.contains("CLAUDE.md"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn build_user_prompt_clips_a_huge_source_file() {
    let dir = scratch("clip");
    fs::write(dir.join("README.md"), "x".repeat(50_000)).unwrap();
    let prompt = build_user_prompt(&read_sources(&dir));
    assert!(prompt.contains("(truncated)"));
    assert!(prompt.len() < 20_000, "prompt was {} chars", prompt.len());
    let _ = fs::remove_dir_all(&dir);
}

// ── rendering ───────────────────────────────────────────────────────────────

#[test]
fn render_produces_frontmatter_and_body() {
    let draft = parse_draft(GOOD_JSON).unwrap();
    let rendered = render_medulla_md(&draft);
    assert!(rendered.starts_with("---\n"));
    assert!(rendered.contains("harnesses: [claude-code, opencode]"));
    assert!(rendered.contains("routing: |"));
    assert!(rendered.contains("  Billing changes -> the payments agent."));
    assert!(rendered.contains("Payments service."));
    // No placeholder survives rendering.
    assert!(!rendered.contains("{{"));
}

#[test]
fn render_of_a_stub_is_still_a_valid_editable_document() {
    let rendered = render_medulla_md(&DraftedProfile::stub());
    assert!(rendered.starts_with("---\n"));
    assert!(rendered.contains("harnesses: []"));
    assert!(rendered.contains("routing: |"));
    assert!(rendered.contains("TODO"));
    assert!(!rendered.contains("{{"));
}

// ── write / read-back ───────────────────────────────────────────────────────

#[test]
fn write_refuses_to_clobber_without_force() {
    let dir = scratch("clobber");
    fs::write(profile_path(&dir), "hand written").unwrap();

    let err = write_medulla_md(&dir, "new", false).unwrap_err();
    assert!(err.to_string().contains("--force"));
    // The authored file is untouched.
    assert_eq!(
        fs::read_to_string(profile_path(&dir)).unwrap(),
        "hand written"
    );

    write_medulla_md(&dir, "new", true).expect("force overwrites");
    assert_eq!(fs::read_to_string(profile_path(&dir)).unwrap(), "new");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn read_medulla_md_round_trips_and_is_none_when_absent() {
    let dir = scratch("roundtrip");
    assert_eq!(read_medulla_md(&dir), None);
    let rendered = render_medulla_md(&parse_draft(GOOD_JSON).unwrap());
    write_medulla_md(&dir, &rendered, false).unwrap();
    assert_eq!(read_medulla_md(&dir).as_deref(), Some(rendered.as_str()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn read_medulla_md_treats_a_blank_file_as_absent() {
    let dir = scratch("blankprofile");
    fs::write(profile_path(&dir), "  \n").unwrap();
    assert_eq!(read_medulla_md(&dir), None);
    let _ = fs::remove_dir_all(&dir);
}

// ── init_workspace ──────────────────────────────────────────────────────────

#[tokio::test]
async fn init_drafts_from_sources_when_a_provider_answers() {
    let dir = scratch("drafted");
    fs::write(dir.join("AGENTS.md"), "billing rules").unwrap();
    let provider = StubProvider(Ok(GOOD_JSON.to_string()));

    let outcome = init_workspace(&dir, Some(&provider), false).await.unwrap();
    assert!(outcome.drafted);
    assert_eq!(outcome.sources, vec!["AGENTS.md"]);
    assert!(outcome.contents.contains("Payments service."));
    assert_eq!(fs::read_to_string(&outcome.path).unwrap(), outcome.contents);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn init_falls_back_to_the_stub_with_no_provider() {
    let dir = scratch("offline");
    fs::write(dir.join("AGENTS.md"), "billing rules").unwrap();

    let outcome = init_workspace(&dir, None, false).await.unwrap();
    assert!(!outcome.drafted);
    assert!(outcome.contents.contains(STUB_SUMMARY));
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn init_falls_back_to_the_stub_when_the_model_fails() {
    let dir = scratch("modelfail");
    fs::write(dir.join("AGENTS.md"), "billing rules").unwrap();
    let provider = StubProvider(Err("upstream 500".to_string()));

    // A provider failure must still leave the operator with a usable file.
    let outcome = init_workspace(&dir, Some(&provider), false).await.unwrap();
    assert!(!outcome.drafted);
    assert!(outcome.contents.contains("TODO"));
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn init_uses_the_stub_when_the_directory_has_no_instruction_files() {
    let dir = scratch("nosources");
    let provider = StubProvider(Ok(GOOD_JSON.to_string()));

    // Nothing to distil, so the model is never called.
    let outcome = init_workspace(&dir, Some(&provider), false).await.unwrap();
    assert!(!outcome.drafted);
    assert!(outcome.sources.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn init_refuses_an_existing_profile_without_force() {
    let dir = scratch("existing");
    fs::write(profile_path(&dir), "hand written").unwrap();
    let provider = StubProvider(Ok(GOOD_JSON.to_string()));

    let err = init_workspace(&dir, Some(&provider), false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("--force"));
    assert_eq!(
        fs::read_to_string(profile_path(&dir)).unwrap(),
        "hand written"
    );
    let _ = fs::remove_dir_all(&dir);
}
