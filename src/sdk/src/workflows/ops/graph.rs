//! Operations on the workflow document itself: reading it, editing it,
//! checking it, and reporting what this host will let it do.
//!
//! Nothing here executes a graph — these are the operations an author reaches
//! for before anything runs.

use serde_json::{json, Value};
use std::sync::Arc;

use super::{parse_ops, record_value};
use crate::workflows::authoring::{
    apply_workflow_ops, create_workflow, preview_workflow_ops, validate_handle, GraphHandle,
};
use crate::workflows::node_contracts::{all_node_kind_contracts, node_kind_contract};
use crate::workflows::store::require;
use crate::workflows::{WorkflowError, WorkflowStore};

/// Every installed workflow, as a listing.
pub fn list(store: &Arc<dyn WorkflowStore>) -> Result<Value, WorkflowError> {
    let workflows = store.list()?;
    Ok(json!({ "workflows": workflows }))
}

/// One workflow, whole: the host fields plus the graph an author would edit.
pub fn get(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    let record = require(store.as_ref(), id)?;
    Ok(record_value(&record))
}

/// Install a workflow from a whole graph document.
pub fn create(
    store: &Arc<dyn WorkflowStore>,
    document: &str,
    id_fallback: &str,
) -> Result<Value, WorkflowError> {
    let record = create_workflow(store, document, id_fallback)?;
    Ok(json!({ "created": record.id, "workflow": record_value(&record) }))
}

/// Set — or clear — a workflow's standing choice of harness and model.
///
/// A separate verb rather than a graph op, because the block is not part of the
/// graph: it is a property of the workflow, and the patch language only speaks
/// nodes and edges. An author who had to rewrite the whole document to pin a
/// harness would lose everything they misremembered on the way.
///
/// `harness` and `model` are each three-valued: absent leaves the current
/// setting alone, a blank string clears it, and anything else sets it. Clearing
/// is spelled that way rather than with `null` because a tool argument that is
/// literally absent and one that is `null` are the same thing to most callers.
///
/// # Errors
///
/// Returns [`WorkflowError`] when the workflow is missing, the harness cannot be
/// one, or the write fails. The stored workflow is left untouched unless the
/// whole change is accepted.
pub fn set_defaults(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    harness: Option<&str>,
    model: Option<&str>,
) -> Result<Value, WorkflowError> {
    let mut record = require(store.as_ref(), id)?;
    if let Some(harness) = harness {
        record.defaults.harness = Some(harness.trim().to_string()).filter(|s| !s.is_empty());
    }
    if let Some(model) = model {
        record.defaults.model = Some(model.trim().to_string()).filter(|s| !s.is_empty());
    }
    record
        .defaults
        .preference()
        .map_err(|message| WorkflowError::Invalid {
            id: id.to_string(),
            messages: vec![format!("`defaults`: {message}")],
        })?;
    store.save(&record)?;
    Ok(json!({ "updated": record.id, "defaults": record.defaults }))
}

/// Remove a workflow.
pub fn delete(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    store.delete(id)?;
    Ok(json!({ "deleted": id }))
}

/// Apply a batch of graph edits and save the result.
///
/// `ops` is the engine's patch language as JSON — an array of tagged objects
/// like `{"op":"update_node_config","id":"build","config":{...}}`.
pub fn apply_ops(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    ops: &Value,
) -> Result<Value, WorkflowError> {
    let ops = parse_ops(ops)?;
    let record = apply_workflow_ops(store, id, &ops)?;
    Ok(json!({ "updated": record.id, "workflow": record_value(&record) }))
}

/// Check a batch of graph edits without saving them.
pub fn preview_ops(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    ops: &Value,
) -> Result<Value, WorkflowError> {
    let ops = parse_ops(ops)?;
    let graph = preview_workflow_ops(store, id, &ops)?;
    Ok(json!({ "ok": true, "graph": graph }))
}

/// Validate a saved workflow, or an inline document that has not been saved.
///
/// Returns `ok: false` with every message rather than an error, because an
/// author asking "is this valid" has been answered either way — a failure here
/// is the result, not a fault.
pub fn validate(store: &Arc<dyn WorkflowStore>, handle: &GraphHandle<'_>) -> Value {
    match validate_handle(store, handle) {
        Ok(record) => json!({
            "ok": true,
            "id": record.id,
            "nodes": record.graph.nodes.len(),
            "edges": record.graph.edges.len(),
        }),
        Err(WorkflowError::Invalid { id, messages }) => {
            json!({ "ok": false, "id": id, "errors": messages })
        }
        Err(err) => json!({ "ok": false, "errors": [err.to_string()] }),
    }
}

/// The node kinds an author may use, with this host's notes.
///
/// The document a model reads before writing a graph. `kind` narrows it to one
/// contract, which is what an author does once they know which node they need.
pub fn catalog(kind: Option<&str>) -> Result<Value, WorkflowError> {
    match kind {
        Some(kind) => {
            let contract = node_kind_contract(kind).ok_or_else(|| {
                WorkflowError::Engine(format!(
                    "unknown node kind '{kind}'; known kinds: {}",
                    crate::workflows::node_contracts::NODE_KINDS.join(", ")
                ))
            })?;
            Ok(json!({ "contract": contract }))
        }
        None => Ok(json!({ "contracts": all_node_kind_contracts() })),
    }
}

/// What a tool session is allowed to report about the machine it runs on.
///
/// The `workflows` config plus the one thing that is not in it: which custom
/// harness presets this machine has. Presets live in their own config section
/// and are loaded from the same layered sources, so they arrive here already
/// resolved rather than being re-read per call.
#[derive(Debug, Clone, Default)]
pub struct HostPolicy {
    /// The operator's `workflows` section.
    pub workflows: crate::config::WorkflowsConfig,
    /// The ids of the custom harness presets configured on this machine, in
    /// config order. An author may name any of these as a node's `harness`.
    pub custom_harnesses: Vec<String>,
}

/// What this host will actually permit a workflow to do.
///
/// The grounding an author most needs and had no way to get. Every one of these
/// is enforced at *run* time, so a graph that ignores them saves cleanly,
/// validates cleanly, and then fails the first time it matters — usually
/// overnight, to nobody watching.
///
/// Read from configuration rather than guessed, so the answer is this machine's
/// and not a plausible default.
pub fn host_facts(policy: &HostPolicy) -> Value {
    let config = &policy.workflows;
    // `run_script` refuses `ScriptLanguage::Shell` on Windows rather than
    // emulating a POSIX shell there (see its doc comment) — an author needs to
    // know that before writing a `medulla:shell`/`code` step, not after it
    // fails on whichever host actually runs the workflow.
    let shell_available = !cfg!(windows);
    json!({
        "defaultWorker": if config.default_worker.trim().is_empty() {
            Value::Null
        } else {
            Value::String(config.default_worker.clone())
        },
        "builtinHarnesses": crate::flow_engine::HarnessSelector::builtin_names(),
        "customHarnesses": policy.custom_harnesses,
        "defaultHarness": config
            .default_provider
            .map(|provider| Value::String(provider.as_str().to_string()))
            .unwrap_or(Value::Null),
        "defaultModel": if config.default_model.trim().is_empty() {
            Value::Null
        } else {
            Value::String(config.default_model.clone())
        },
        "nativeTools": crate::flow_engine::caps::tools::NATIVE_TOOLS,
        "toolAllowlist": config.tool_allowlist,
        "httpAllowlist": config.http_allowlist,
        "allowCode": config.allow_code,
        "runTimeoutSecs": config.run_timeout_secs,
        "shellScriptsAvailable": shell_available,
        "notes": [
            if config.default_worker.trim().is_empty() {
                "No default worker is configured, so every `agent` node must name a \
                 `config.agent_ref` — a node without one fails at run time."
            } else {
                "An `agent` node with no `config.agent_ref` dispatches to the default worker \
                 above. Any other value must name a worker this host can actually reach."
            },
            "An `agent` node may name its own `config.harness` and `config.model`, and a \
             workflow document may set a `defaults` block for every node in it. Anything named \
             there overrides `defaultHarness`/`defaultModel` above. A harness outside \
             `builtinHarnesses` is taken as a custom preset id — `customHarnesses` lists the ones \
             this machine has configured, though the worker that runs the step is the one that \
             has to expose it.",
            "A `tool_call` slug outside `nativeTools` must appear in `toolAllowlist`, and even \
             then there is no third-party integration registry on this host — it will still \
             fail at run time. Express host-specific work as an `agent` node.",
            "Only `manual` triggers fire here. Other kinds are stored and never dispatched.",
            if shell_available {
                "`medulla:shell` may use `language: shell` here."
            } else {
                "This host is Windows: `medulla:shell` must use `language: javascript` or \
                 `language: python` — `shell` is refused, not emulated. (A `code` node never \
                 accepts `shell` at all, on any host.)"
            },
        ],
    })
}
