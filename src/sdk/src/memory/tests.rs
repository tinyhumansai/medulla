//! Unit tests for the memory service: status/overview rendering, the provider
//! selection ladder, offline compile, and the report translations.

use super::*;
use std::path::PathBuf;

fn settings(workspace: PathBuf) -> MemorySettings {
    MemorySettings {
        enabled: true,
        workspace,
        identity: "test@example.com".into(),
        claude_root: None,
        codex_root: None,
        project_roots: Vec::new(),
        llm_model: None,
        max_cost_usd: 5.0,
        openrouter_api_key: None,
        backend: None,
    }
}

#[test]
fn status_on_empty_workspace_is_zeroed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = MemoryService::open(settings(tmp.path().to_path_buf())).unwrap();
    let status = svc.status();
    assert!(status.enabled);
    assert!(!status.pack_exists);
    assert_eq!(status.entry_count, 0);
    assert_eq!(status.directives_count, 0);
    assert!(status.facet_counts.is_empty());
    assert!(svc.search("anything", None, 5).is_empty());
    assert!(svc.pack_path().ends_with("PERSONA.md"));
}

#[tokio::test]
async fn ingest_without_credentials_errors_clearly() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = MemoryService::open(settings(tmp.path().to_path_buf())).unwrap();
    let err = svc.ingest(IngestMode::Incremental).await.unwrap_err();
    assert!(err.to_string().contains("medulla login"));
}

#[test]
fn provider_prefers_openrouter_key_then_backend() {
    let tmp = tempfile::TempDir::new().unwrap();
    // No key, no backend → clear error.
    let svc = MemoryService::open(settings(tmp.path().to_path_buf())).unwrap();
    assert!(svc.provider().is_err());
    // Backend only → ok (summarization model via the backend surface).
    let svc =
        MemoryService::open(settings(tmp.path().to_path_buf()).with_backend("http://b:1/", "jwt"))
            .unwrap();
    assert!(svc.provider().is_ok());
    // Explicit key also ok.
    let mut s = settings(tmp.path().to_path_buf());
    s.openrouter_api_key = Some("sk-x".into());
    let svc = MemoryService::open(s).unwrap();
    assert!(svc.provider().is_ok());
}

#[test]
fn compile_offline_writes_a_pack_and_reloads() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Exercise every optional persona_config branch (roots + model) too.
    let mut s = settings(tmp.path().to_path_buf());
    s.claude_root = Some(tmp.path().join("claude"));
    s.codex_root = Some(tmp.path().join("codex"));
    s.project_roots = vec![tmp.path().join("proj")];
    s.llm_model = Some("test/model".into());
    let svc = MemoryService::open(s).unwrap();
    // No LLM is called on the compile-only path, so this runs fully offline.
    let report = svc.compile().unwrap();
    assert_eq!(report.mode, "compile");
    assert!(report.pack_path.is_some());
    // The provider builder honors the explicit llm_model override.
    let mut s2 = settings(tmp.path().to_path_buf());
    s2.llm_model = Some("custom/model".into());
    s2.openrouter_api_key = Some("sk-x".into());
    assert!(MemoryService::open(s2).unwrap().provider().is_ok());
}

#[test]
fn overview_renders_disabled_and_facets() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut s = settings(tmp.path().to_path_buf());
    s.enabled = false;
    let svc = MemoryService::open(s).unwrap();
    let text = svc.overview();
    assert!(text.contains("memory: disabled"));
    assert!(text.contains("facets: (none)"));
}

#[test]
fn overview_renders_enabled_header() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Default settings are enabled; the pack is absent on an empty workspace.
    let svc = MemoryService::open(settings(tmp.path().to_path_buf())).unwrap();
    let text = svc.overview();
    assert!(text.contains("memory: enabled"));
    assert!(text.contains("(absent)"));
}

#[test]
fn ingest_mode_maps_to_run_mode() {
    assert!(matches!(
        IngestMode::Backfill.into_run_mode(),
        RunMode::Backfill
    ));
    assert!(matches!(
        IngestMode::Incremental.into_run_mode(),
        RunMode::Incremental
    ));
}

#[test]
fn from_run_report_translates_all_fields() {
    let mut report = tinycortex::memory::persona::RunReport {
        mode: "backfill".into(),
        files_seen: 3,
        sessions_processed: 2,
        sessions_skipped: 1,
        sessions_failed: 1,
        directives_folded: 4,
        observations: 7,
        budget_hit: true,
        pack_path: Some("/tmp/PERSONA.md".into()),
        ..Default::default()
    };
    report.facet_counts.insert("coding_style".into(), 5);

    let out = from_run_report(report);
    assert_eq!(out.mode, "backfill");
    assert_eq!(out.files_seen, 3);
    assert_eq!(out.sessions_processed, 2);
    assert_eq!(out.sessions_skipped, 1);
    assert_eq!(out.sessions_failed, 1);
    assert_eq!(out.directives_folded, 4);
    assert_eq!(out.observations, 7);
    assert!(out.budget_hit);
    assert_eq!(out.pack_path.as_deref(), Some("/tmp/PERSONA.md"));
    assert_eq!(out.facet_counts.get("coding_style"), Some(&5));
}

#[tokio::test]
async fn noop_provider_errors_on_chat() {
    let provider = NoopProvider;
    assert_eq!(provider.name(), "noop");
    let prompt = ChatPrompt {
        system: String::new(),
        user: String::new(),
        temperature: 0.0,
        kind: "test",
        max_tokens: None,
    };
    assert!(provider.chat_for_json(&prompt).await.is_err());
}
