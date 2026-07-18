//! End-to-end tests for the persona-memory ↔ core-js wire: the [`CoreRuntime`]
//! advertises the memory toolset at `initialize`, then serves core-issued
//! `memory_query` events from an attached [`MemoryService`], replying via the
//! `memory.answer` RPC. Uses the configurable [`mock_core`] stub and a real
//! (offline-seeded) persona workspace.

mod support;

#[path = "support/mock_core.rs"]
mod mock_core;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use medulla::memory::{MemoryService, MemorySettings};
use medulla::runtime::core::CoreRuntime;
use medulla::runtime::core_client::CoreClient;

use mock_core::{MockCore, MockCoreConfig};
use support::wait_until;

use tinycortex::memory::config::MemoryConfig;
use tinycortex::memory::persona::compile::write_directives;
use tinycortex::memory::persona::reduce::{fold_digest, FacetAsks, ReduceState};
use tinycortex::memory::persona::types::{
    DigestObservation, EvidenceSource, EvidenceTier, PersonaFacet, PersonaSourceKind, SessionDigest,
};
use tinycortex::memory::tree::summarise::ConcatSummariser;

const T: Duration = Duration::from_secs(5);

fn settings(workspace: std::path::PathBuf, enabled: bool) -> MemorySettings {
    MemorySettings {
        enabled,
        workspace,
        identity: "dev@example.com".into(),
        claude_root: None,
        codex_root: None,
        project_roots: Vec::new(),
        llm_model: None,
        max_cost_usd: 5.0,
        openrouter_api_key: None,
        backend: None,
    }
}

async fn seed(workspace: &Path) {
    let config = MemoryConfig::new(workspace);
    let mut state = ReduceState::default();
    let digest = SessionDigest {
        source: EvidenceSource::new(PersonaSourceKind::ClaudeCode).with_scope("medulla"),
        observations: vec![DigestObservation {
            facet: PersonaFacet::CodingStyle,
            observation: "Keep modules under 500 lines".to_string(),
            quote: String::new(),
            tier: EvidenceTier::T0,
        }],
    };
    fold_digest(
        &config,
        &digest,
        &FacetAsks::default(),
        &ConcatSummariser::new(),
        &mut state,
    )
    .await
    .unwrap();
    write_directives(&config, &["[global] Test before handoff.".to_string()]).unwrap();
}

fn tmp_sock() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("core.sock");
    (dir, sock)
}

async fn connect_with(sock: &Path, service: Option<Arc<MemoryService>>) -> CoreRuntime {
    let (client, rx) = CoreClient::connect(sock).await.unwrap();
    CoreRuntime::connect(client, rx, "test", service)
        .await
        .unwrap()
}

// initialize advertises the memory capability (tool names + pack path), and a
// core-issued `memory_search` query is served back via `memory.answer`.
#[tokio::test]
async fn advertises_and_serves_memory_search() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path()).await;
    let service = Arc::new(MemoryService::open(settings(ws.path().to_path_buf(), true)).unwrap());
    let pack_path = service.pack_path();

    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start_with(&sock, MockCoreConfig::default()).await;
    let _rt = connect_with(&sock, Some(service)).await;

    // The handshake carried the memory capability.
    let init = mock.params_of("initialize").expect("initialize called");
    let capability = init.get("memory").expect("memory capability advertised");
    let tools: Vec<&str> = capability
        .get("tools")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(tools.contains(&"memory_search"));
    assert!(tools.contains(&"memory_directives"));
    assert_eq!(
        capability.get("packPath").and_then(Value::as_str),
        Some(pack_path.as_str())
    );

    // The core asks for a search; the runtime answers.
    mock.push_event(
        1,
        "cyc:app:th_test:1",
        json!({
            "kind": "memory_query",
            "id": "q1",
            "tool": "memory_search",
            "params": {"query": "lines per module file", "k": 3}
        }),
    );

    wait_until("memory.answer served", T, || {
        mock.calls().contains(&"memory.answer".to_string())
    })
    .await;

    let answer = mock.params_of("memory.answer").unwrap();
    assert_eq!(answer.get("id").and_then(Value::as_str), Some("q1"));
    let hits = answer
        .get("ok")
        .and_then(|ok| ok.get("hits"))
        .and_then(Value::as_array)
        .expect("hits array");
    assert!(!hits.is_empty());
    assert_eq!(
        hits[0].get("facet").and_then(Value::as_str),
        Some("coding_style")
    );
}

// An unknown tool name yields an error answer, not an ok payload.
#[tokio::test]
async fn unknown_tool_yields_error_answer() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path()).await;
    let service = Arc::new(MemoryService::open(settings(ws.path().to_path_buf(), true)).unwrap());

    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start_with(&sock, MockCoreConfig::default()).await;
    let _rt = connect_with(&sock, Some(service)).await;

    mock.push_event(
        1,
        "cyc:app:th_test:1",
        json!({"kind": "memory_query", "id": "q9", "tool": "bogus_tool"}),
    );

    wait_until("error answer served", T, || {
        mock.calls().contains(&"memory.answer".to_string())
    })
    .await;

    let answer = mock.params_of("memory.answer").unwrap();
    assert_eq!(answer.get("id").and_then(Value::as_str), Some("q9"));
    assert!(answer.get("ok").is_none());
    let msg = answer
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(msg.contains("unknown memory tool"), "got: {msg}");
}

// A disabled service is never attached: no advertise, and `memory_query` events
// are ignored (no `memory.answer`).
#[tokio::test]
async fn disabled_service_neither_advertises_nor_serves() {
    let ws = tempfile::TempDir::new().unwrap();
    let service = Arc::new(MemoryService::open(settings(ws.path().to_path_buf(), false)).unwrap());

    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start_with(&sock, MockCoreConfig::default()).await;
    let _rt = connect_with(&sock, Some(service)).await;

    let init = mock.params_of("initialize").expect("initialize called");
    assert!(init.get("memory").is_none(), "disabled must not advertise");

    mock.push_event(
        1,
        "cyc:app:th_test:1",
        json!({"kind": "memory_query", "id": "q1", "tool": "memory_search", "params": {}}),
    );
    // Give the fold loop a moment; no answer should ever be produced.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!mock.calls().contains(&"memory.answer".to_string()));
}
