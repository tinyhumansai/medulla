//! End-to-end workspace-initialisation tests: the full `medulla init` flow over
//! a real directory tree, from reading instruction files through writing,
//! reading back, and building the run-request payload.
//!
//! Offline and deterministic, which is now the only path there is — the
//! model-drafted body went out with the memory layer that owned the provider
//! seam, so the profile body is a stub the operator edits.

use std::fs;
use std::path::PathBuf;

use medulla::init::{collect_profile_inputs, init_workspace, profile_path, read_medulla_md};

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

#[tokio::test]
async fn init_reads_sources_and_writes_a_profile() {
    let dir = repo(
        "full",
        &[
            ("AGENTS.md", "Run npm test before handoff."),
            ("README.md", "# Payments\n\nBilling and Stripe webhooks."),
        ],
    );

    let outcome = init_workspace(&dir, false).await.unwrap();

    // Both instruction files are reported, even though nothing distils them:
    // the operator is told what the directory carries.
    assert!(!outcome.drafted);
    assert_eq!(outcome.sources, vec!["AGENTS.md", "README.md"]);

    // The file on disk is the rendered document, with an editable stub body.
    let written = fs::read_to_string(profile_path(&dir)).unwrap();
    assert_eq!(written, outcome.contents);
    assert!(written.starts_with("---\n"));
    assert!(written.contains("TODO"));
    assert!(!written.contains("{{"), "no placeholder survives rendering");

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_written_profile_reads_back_and_becomes_a_run_request_payload() {
    let dir = repo("payload", &[("AGENTS.md", "house rules")]);
    init_workspace(&dir, false).await.unwrap();

    let text = read_medulla_md(&dir).expect("profile reads back");

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
    init_workspace(&with, false).await.unwrap();

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
async fn re_running_init_preserves_an_edited_profile_unless_forced() {
    let dir = repo("preserve", &[("AGENTS.md", "house rules")]);
    init_workspace(&dir, false).await.unwrap();

    // The operator edits the profile by hand — which is the whole point of
    // writing a stub.
    let edited = "---\nharnesses: [opencode]\n---\n\nHand-tuned summary.";
    fs::write(profile_path(&dir), edited).unwrap();

    // A re-run must not silently discard that edit.
    let err = init_workspace(&dir, false).await.unwrap_err();
    assert!(err.to_string().contains("--force"));
    assert_eq!(fs::read_to_string(profile_path(&dir)).unwrap(), edited);

    // --force overwrites deliberately.
    let outcome = init_workspace(&dir, true).await.unwrap();
    assert!(outcome.contents.contains("TODO"));
    assert_eq!(
        fs::read_to_string(profile_path(&dir)).unwrap(),
        outcome.contents
    );

    let _ = fs::remove_dir_all(&dir);
}
