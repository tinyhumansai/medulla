//! Unit tests for workspace initialisation. Every test is offline and
//! deterministic — which is now the only path there is: the model-drafted body
//! went out with the memory layer that owned the provider seam.

use std::fs;
use std::path::PathBuf;

use super::*;
use crate::init::types::STUB_SUMMARY;

mod layout;
mod registry;

/// A unique scratch directory per test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("medulla-init-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A populated draft, standing in for what an authored profile carries once an
/// operator has filled the stub in.
fn authored() -> DraftedProfile {
    DraftedProfile {
        summary: "Payments service. Owns billing, invoices, and the Stripe integration."
            .to_string(),
        harnesses: vec!["claude-code".to_string(), "opencode".to_string()],
        models_reasoning: Vec::new(),
        routing: vec!["Billing changes -> the payments agent.".to_string()],
    }
}

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

// ── rendering ───────────────────────────────────────────────────────────────

#[test]
fn render_produces_frontmatter_and_body() {
    let rendered = render_medulla_md(&authored(), &[]);
    assert!(rendered.starts_with("---\n"));
    assert!(rendered.contains("harnesses: [claude-code, opencode]"));
    assert!(rendered.contains("routing: |"));
    assert!(rendered.contains("  Billing changes -> the payments agent."));
    assert!(rendered.contains("Payments service."));
    // No placeholder survives rendering.
    assert!(!rendered.contains("{{"));
}

#[test]
fn render_never_emits_carriage_returns() {
    // A Windows checkout can embed the template with CRLF. The rendered file is
    // parsed and shipped over the wire, so it must be LF on every platform —
    // this is what broke the Windows CI job.
    for draft in [authored(), DraftedProfile::stub()] {
        let rendered = render_medulla_md(&draft, &[]);
        assert!(!rendered.contains('\r'), "rendered document contained CR");
        assert!(rendered.starts_with("---\n"));
    }
}

#[test]
fn render_of_a_stub_is_still_a_valid_editable_document() {
    let rendered = render_medulla_md(&DraftedProfile::stub(), &[]);
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
    let rendered = render_medulla_md(&authored(), &[]);
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
async fn init_writes_the_stub_and_reports_the_sources_it_saw() {
    let dir = scratch("stub");
    fs::write(dir.join("AGENTS.md"), "billing rules").unwrap();

    let outcome = init_workspace(&dir, false).await.unwrap();
    // Never drafted: the body is the stub, and the sources are still reported
    // so the operator knows what the layout scan looked at.
    assert!(!outcome.drafted);
    assert_eq!(outcome.sources, vec!["AGENTS.md"]);
    assert!(outcome.contents.contains(STUB_SUMMARY));
    assert_eq!(fs::read_to_string(&outcome.path).unwrap(), outcome.contents);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn init_on_a_directory_with_no_instruction_files_still_writes_one() {
    let dir = scratch("nosources");

    let outcome = init_workspace(&dir, false).await.unwrap();
    assert!(outcome.sources.is_empty());
    assert!(outcome.contents.contains("TODO"));
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn init_refuses_an_existing_profile_without_force() {
    let dir = scratch("existing");
    fs::write(profile_path(&dir), "hand written").unwrap();

    let err = init_workspace(&dir, false).await.unwrap_err();
    assert!(err.to_string().contains("--force"));
    assert_eq!(
        fs::read_to_string(profile_path(&dir)).unwrap(),
        "hand written"
    );
    let _ = fs::remove_dir_all(&dir);
}
