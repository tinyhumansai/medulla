//! The on-disk workflow document: reading it, writing it, checking it.
//!
//! A document is the engine's `WorkflowGraph` JSON with this host's own fields
//! (`id`, `name`, `description`, `enabled`) merged in beside it, rather than
//! nested under a wrapper. That shape is deliberate: a file an operator opens
//! reads as a graph, and a graph exported from anywhere else loads here without
//! being re-wrapped.

use std::path::Path;

use serde_json::Value;
use tinyflows::model::WorkflowGraph;

use crate::workflows::types::{RunRecord, RunStatus, WorkflowError, WorkflowRecord};

/// Read and parse one workflow document, naming errors by path.
pub fn read_workflow(path: &Path) -> Result<WorkflowRecord, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let mut record =
        parse_workflow(&text, stem).map_err(|err| format!("{}: {err}", path.display()))?;
    // A document can deserialize cleanly and still be a graph the engine will
    // not compile — no trigger, an edge to a node that is not there. Catching
    // it here means a listing only ever shows workflows that would actually
    // run, and the operator hears about the broken file by name.
    validate_graph(&record.id, &record.graph)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    record.source_path = Some(path.to_path_buf());
    Ok(record)
}

/// Parse one workflow document.
///
/// The document is the engine's `WorkflowGraph` JSON with optional host fields
/// (`description`, `enabled`) alongside it. `id` defaults to `id_fallback` — the
/// filename, for a file — and `name` to the id, so the smallest useful document
/// is a set of nodes and edges.
///
/// The pipeline is the engine's documented one: migrate the persisted JSON to
/// the current schema *before* deserializing, so a definition saved by an older
/// build keeps loading.
pub fn parse_workflow(text: &str, id_fallback: &str) -> Result<WorkflowRecord, String> {
    let raw: Value = serde_json::from_str(text).map_err(|err| format!("invalid JSON: {err}"))?;
    let migrated = tinyflows::migrate::migrate(raw).map_err(|err| err.to_string())?;

    let object = migrated
        .as_object()
        .ok_or_else(|| "workflow document must be a JSON object".to_string())?;

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(id_fallback)
        .to_string();
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let enabled = object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let graph: WorkflowGraph =
        serde_json::from_value(migrated).map_err(|err| format!("invalid workflow: {err}"))?;
    let name = if graph.name.is_empty() {
        id.clone()
    } else {
        graph.name.clone()
    };

    Ok(WorkflowRecord {
        id,
        name,
        description,
        enabled,
        graph,
        source_path: None,
    })
}

/// Run the engine's validation, collecting every failure rather than the first.
///
/// One round-trip then tells an author everything wrong with their graph, which
/// matters most when the author is an agent editing over a tool call.
pub fn validate_graph(id: &str, graph: &WorkflowGraph) -> Result<(), WorkflowError> {
    let errors = tinyflows::validate::validate_all(graph);
    if errors.is_empty() {
        return Ok(());
    }
    Err(WorkflowError::Invalid {
        id: id.to_string(),
        messages: errors
            .iter()
            .map(|err| match err.node_id() {
                Some(node) => format!("[{}] {node}: {err}", err.code()),
                None => format!("[{}] {err}", err.code()),
            })
            .collect(),
    })
}

/// Serialize a record into the on-disk document shape: the graph, with the host
/// fields merged in beside it.
pub fn to_document(record: &WorkflowRecord) -> Result<Vec<u8>, WorkflowError> {
    let mut value = serde_json::to_value(&record.graph)
        .map_err(|err| WorkflowError::Malformed(err.to_string()))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("id".into(), Value::String(record.id.clone()));
        object.insert("name".into(), Value::String(record.name.clone()));
        object.insert(
            "description".into(),
            Value::String(record.description.clone()),
        );
        object.insert("enabled".into(), Value::Bool(record.enabled));
    }
    serde_json::to_vec_pretty(&value).map_err(|err| WorkflowError::Malformed(err.to_string()))
}

/// A run record for a run that has just started.
pub fn new_run_record(id: &str, workflow_id: &str, started_at: u64) -> RunRecord {
    RunRecord {
        id: id.to_string(),
        workflow_id: workflow_id.to_string(),
        status: RunStatus::Running,
        started_at,
        finished_at: None,
        steps: Vec::new(),
        pending_approvals: Vec::new(),
        error: None,
        // Both are evidence about a run that has ended, so a run that has only
        // just started has neither. They are filled in when it settles.
        summary: None,
        diagnosis: None,
    }
}
