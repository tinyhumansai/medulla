//! End-to-end memory-service tests over a real tinycortex persona workspace.
//!
//! The workspace is built offline by folding hand-authored `SessionDigest`s with
//! the deterministic `ConcatSummariser` (mirroring the vendored persona tests) —
//! no network, no LLM — then exercised through medulla's [`MemoryService`].

use std::path::PathBuf;

use medulla::memory::{MemoryService, MemorySettings};

use tinycortex::memory::config::MemoryConfig;
use tinycortex::memory::persona::compile::write_directives;
use tinycortex::memory::persona::reduce::{fold_digest, FacetAsks, ReduceState};
use tinycortex::memory::persona::types::{
    DigestObservation, EvidenceSource, PersonaFacet, PersonaSourceKind, SessionDigest,
};
use tinycortex::memory::tree::summarise::ConcatSummariser;

fn settings(workspace: PathBuf) -> MemorySettings {
    MemorySettings {
        enabled: true,
        workspace,
        identity: "dev@example.com".into(),
        claude_root: None,
        codex_root: None,
        project_roots: Vec::new(),
        llm_model: None,
        max_cost_usd: 5.0,
        openrouter_api_key: None,
    }
}

/// Fold two facet observations + a verbatim directive into `workspace`.
async fn seed_workspace(workspace: &std::path::Path) {
    let config = MemoryConfig::new(workspace);
    let summariser = ConcatSummariser::new();
    let mut state = ReduceState::default();

    let digest = SessionDigest {
        source: EvidenceSource::new(PersonaSourceKind::ClaudeCode).with_scope("medulla"),
        observations: vec![
            DigestObservation {
                facet: PersonaFacet::CodingStyle,
                observation: "Keep modules under 500 lines and split before that".to_string(),
                quote: "avoid letting any source file grow beyond 500 lines".to_string(),
                tier: tinycortex::memory::persona::types::EvidenceTier::T0,
            },
            DigestObservation {
                facet: PersonaFacet::Workflow,
                observation: "Branch before writing code; never commit to main".to_string(),
                quote: String::new(),
                tier: tinycortex::memory::persona::types::EvidenceTier::T1,
            },
        ],
    };

    fold_digest(
        &config,
        &digest,
        &FacetAsks::default(),
        &summariser,
        &mut state,
    )
    .await
    .unwrap();

    write_directives(
        &config,
        &["[global] Always run the test suite before handoff.".to_string()],
    )
    .unwrap();
}

#[tokio::test]
async fn service_search_status_and_directives_over_real_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    seed_workspace(tmp.path()).await;

    let svc = MemoryService::open(settings(tmp.path().to_path_buf())).unwrap();

    // Status reflects the folded corpus + persisted directives.
    let status = svc.status();
    assert!(status.enabled);
    assert_eq!(status.entry_count, 2);
    assert_eq!(status.directives_count, 1);
    assert_eq!(
        status.facet_counts.get("coding_style").copied(),
        Some(1),
        "coding_style facet folded"
    );
    assert_eq!(status.facet_counts.get("workflow").copied(), Some(1));

    // Directives come back verbatim.
    let directives = svc.directives();
    assert_eq!(directives.len(), 1);
    assert!(directives[0].contains("test suite before handoff"));

    // Lexical search ranks the relevant observation first, translated to the
    // medulla-owned hit type (facet/tier as strings, RFC3339 timestamp).
    let hits = svc.search("how many lines per module file", None, 5);
    assert!(!hits.is_empty());
    assert_eq!(hits[0].facet, "coding_style");
    assert_eq!(hits[0].tier, "t0");
    assert!(hits[0].text.contains("500 lines"));
    assert!(hits[0].timestamp.contains('T'));

    // A facet filter restricts results; an unknown facet name is ignored.
    let workflow_only = svc.search("commit", Some("workflow"), 5);
    assert!(workflow_only.iter().all(|h| h.facet == "workflow"));
    let unknown_facet = svc.search("commit", Some("not-a-facet"), 5);
    assert!(
        !unknown_facet.is_empty(),
        "an unrecognized facet must not filter everything out"
    );
}

#[tokio::test]
async fn reload_picks_up_a_freshly_seeded_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = MemoryService::open(settings(tmp.path().to_path_buf())).unwrap();
    // Empty before seeding (retriever caches the empty load).
    assert_eq!(svc.status().entry_count, 0);

    seed_workspace(tmp.path()).await;
    // Still cached as empty until reload.
    assert_eq!(svc.status().entry_count, 0);
    svc.reload();
    assert_eq!(svc.status().entry_count, 2);
}
