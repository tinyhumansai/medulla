//! Persona-memory tool surface for the core runtime: the tool names advertised at
//! `initialize`, the capability object that carries them, and the handler that
//! serves one `memory_query` event against the attached [`MemoryService`].

use serde_json::{json, Value};

use crate::memory::MemoryService;

/// The persona-memory tools advertised to the reasoning layer.
pub const MEMORY_TOOLS: [&str; 4] = [
    "memory_search",
    "memory_directives",
    "memory_overview",
    "memory_status",
];

/// Build the memory capability object advertised at `initialize`: the tool names
/// plus the compiled `PERSONA.md` pack path.
pub(super) fn memory_capability(service: &MemoryService) -> Value {
    json!({
        "tools": MEMORY_TOOLS,
        "packPath": service.pack_path(),
    })
}

/// Serve one `memory_query` event body against the attached [`MemoryService`].
/// Returns `(id, result)` where `result` is the tool payload or an error string.
/// An unknown tool is an error answer.
pub(super) fn serve_memory_query(
    service: &MemoryService,
    body: &Value,
) -> (String, Result<Value, String>) {
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let tool = body.get("tool").and_then(Value::as_str).unwrap_or("");
    let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match tool {
        "memory_search" => {
            let query = params.get("query").and_then(Value::as_str).unwrap_or("");
            let facet = params
                .get("facet")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let k = params.get("k").and_then(Value::as_u64).unwrap_or(5) as usize;
            let hits = service.search(query, facet.as_deref(), k);
            Ok(json!({ "hits": hits }))
        }
        "memory_directives" => Ok(json!({ "directives": service.directives() })),
        "memory_overview" => Ok(json!({ "overview": service.overview() })),
        "memory_status" => Ok(json!({ "status": service.status() })),
        other => Err(format!("unknown memory tool '{other}'")),
    };
    (id, result)
}
