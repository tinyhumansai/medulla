//! End-to-end workspace-initialisation tests: the full `medulla init` flow over
//! a real directory tree, from reading instruction files through drafting,
//! writing, reading back, and building the run-request payload.
//!
//! Offline and deterministic — the drafting path runs through a stub
//! `ChatProvider`, never a live model.

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Result};

use medulla::init::{collect_profile_inputs, init_workspace, profile_path, read_medulla_md};

use tinycortex::memory::score::extract::{ChatPrompt, ChatProvider};

/// A scratch repo with the given instruction files.
fn repo(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("medulla-feature-init-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch repo");
    for (name, body) in files {
        fs::write(dir.join(name), body).expect("write source");
    }
    dir
}

/// Records the prompt it was given, then answers with a canned draft.
struct RecordingProvider {
    response: Result<String, String>,
    seen: std::sync::Mutex<Vec<String>>,
}

impl RecordingProvider {
    fn ok(json: &str) -> Self {
        RecordingProvider {
            response: Ok(json.to_string()),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }
    fn failing() -> Self {
        RecordingProvider {
            response: Err("upstream unavailable".into()),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }
    fn prompts(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ChatProvider for RecordingProvider {
    fn name(&self) -> &str {
        "recording"
    }
    async fn chat_for_json(&self, prompt: &ChatPrompt) -> Result<String> {
        self.seen.lock().unwrap().push(prompt.user.clone());
        self.response.clone().map_err(|e| anyhow!(e))
    }
}

const DRAFT: &str = r#"{
  "summary": "Payments service. Owns billing, invoices, and the Stripe integration.",
  "harnesses": ["claude-code"],
  "models_reasoning": ["claude-opus-4-8"],
  "routing": ["Billing changes -> the payments agent.", "Migrations need review."]
}"#;

#[tokio::test]
async fn init_reads_sources_drafts_and_writes_a_profile() {
    let dir = repo(
        "full",
        &[
            ("AGENTS.md", "Run npm test before handoff."),
            ("README.md", "# Payments\n\nBilling and Stripe webhooks."),
        ],
    );
    let provider = RecordingProvider::ok(DRAFT);

    let outcome = init_workspace(&dir, Some(&provider), false).await.unwrap();

    // The model saw both instruction files.
    let prompts = provider.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Run npm test before handoff."));
    assert!(prompts[0].contains("Billing and Stripe webhooks."));

    assert!(outcome.drafted);
    assert_eq!(outcome.sources, vec!["AGENTS.md", "README.md"]);

    // The file on disk is the rendered document, with the drafted fields in it.
    let written = fs::read_to_string(profile_path(&dir)).unwrap();
    assert_eq!(written, outcome.contents);
    assert!(written.contains("harnesses: [claude-code]"));
    assert!(written.contains("reasoning: [claude-opus-4-8]"));
    assert!(written.contains("  Billing changes -> the payments agent."));
    assert!(written.contains("  Migrations need review."));
    assert!(written.contains("Payments service."));

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_written_profile_reads_back_and_becomes_a_run_request_payload() {
    let dir = repo("payload", &[("AGENTS.md", "house rules")]);
    let provider = RecordingProvider::ok(DRAFT);
    init_workspace(&dir, Some(&provider), false).await.unwrap();

    let text = read_medulla_md(&dir).expect("profile reads back");
    assert!(text.contains("Payments service."));

    // The forward payload carries the directory path and the verbatim text.
    let inputs = collect_profile_inputs(std::slice::from_ref(&dir));
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].workspace, dir.display().to_string());
    assert_eq!(inputs[0].medulla_md, text);

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn collect_skips_directories_without_a_profile() {
    let with = repo("collect-with", &[("AGENTS.md", "rules")]);
    let without = repo("collect-without", &[("AGENTS.md", "rules")]);
    init_workspace(&with, None, false).await.unwrap();

    let inputs =
        collect_profile_inputs(&[with.clone(), without.clone(), PathBuf::from("/no/such")]);
    assert_eq!(
        inputs.len(),
        1,
        "only the initialised directory contributes"
    );
    assert_eq!(inputs[0].workspace, with.display().to_string());

    let _ = fs::remove_dir_all(&with);
    let _ = fs::remove_dir_all(&without);
}

#[tokio::test]
async fn collect_over_no_directories_is_empty() {
    assert!(collect_profile_inputs(&[]).is_empty());
}

#[tokio::test]
async fn a_failing_model_still_leaves_a_usable_profile() {
    let dir = repo("degrade", &[("AGENTS.md", "house rules")]);
    let provider = RecordingProvider::failing();

    let outcome = init_workspace(&dir, Some(&provider), false).await.unwrap();

    assert!(!outcome.drafted, "the draft failed");
    // Still a real file, and still round-trips into a payload.
    assert!(read_medulla_md(&dir).is_some());
    assert_eq!(collect_profile_inputs(std::slice::from_ref(&dir)).len(), 1);
    assert!(outcome.contents.contains("TODO"));

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn re_running_init_preserves_an_edited_profile_unless_forced() {
    let dir = repo("preserve", &[("AGENTS.md", "house rules")]);
    let provider = RecordingProvider::ok(DRAFT);
    init_workspace(&dir, Some(&provider), false).await.unwrap();

    // The operator edits the drafted profile by hand.
    let edited = "---\nharnesses: [opencode]\n---\n\nHand-tuned summary.";
    fs::write(profile_path(&dir), edited).unwrap();

    // A re-run must not silently discard that edit — and must not spend a call.
    let before = provider.prompts().len();
    let err = init_workspace(&dir, Some(&provider), false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("--force"));
    assert_eq!(fs::read_to_string(profile_path(&dir)).unwrap(), edited);
    assert_eq!(
        provider.prompts().len(),
        before,
        "the existence check must short-circuit before the model call"
    );

    // --force overwrites deliberately.
    let outcome = init_workspace(&dir, Some(&provider), true).await.unwrap();
    assert!(outcome.drafted);
    assert!(fs::read_to_string(profile_path(&dir))
        .unwrap()
        .contains("Payments service."));

    let _ = fs::remove_dir_all(&dir);
}
