//! The node-kind catalogue, with this host's facts layered on.
//!
//! The engine publishes a contract per node kind — config fields, ports, an
//! example, and the gotchas that bite in practice. It is deliberately
//! host-agnostic, so it cannot say that `agent_ref` here names a *worker*, that
//! `tool_call` slugs are `medulla:`-prefixed, or that `code` nodes are refused
//! unless an operator turned them on.
//!
//! Rather than fork the catalogue, this module keeps the engine's contracts and
//! appends host notes ([`apply_host_overlay`]). An author — usually an agent
//! reading this over a tool call — gets one document that is both accurate about
//! the engine and accurate about where it is running.

use tinyflows::catalog::NodeKindContract;

pub use tinyflows::catalog::{ConfigField, PortSpec, NODE_KINDS};

use crate::flow_engine::caps::tools::{NATIVE_TOOLS, NATIVE_TOOL_PREFIX};

/// Add this host's notes to one engine contract.
///
/// Additive only: nothing the engine says is edited or removed, so a contract
/// stays true if the overlay is ever dropped.
pub fn apply_host_overlay(mut contract: NodeKindContract) -> NodeKindContract {
    let notes: Vec<String> = match contract.kind.as_str() {
        "agent" => vec![
            "On Medulla an agent node is a task dispatched to a coding harness (Claude Code, \
             Codex, OpenCode) over the worker bridge — not a single model call. The node \
             completes when the harness session does."
                .to_string(),
            "`config.agent_ref` names the worker to dispatch to. Omit it to use the \
             `workflows.defaultWorker` from config; a run with neither fails and says so."
                .to_string(),
            "`config.prompt` carries the instruction. `instruction` is accepted as an alias, \
             since that is what the rest of Medulla calls it."
                .to_string(),
            "The harness reply is available as `=item.text`. If the harness replied with JSON \
             it is also parsed, so `=item.json.<field>` works without an output_parser."
                .to_string(),
        ],
        "tool_call" => vec![
            format!(
                "Slugs beginning `{NATIVE_TOOL_PREFIX}` are built into this host: {}. Any other \
                 slug must be listed in `workflows.toolAllowlist`, and is refused otherwise.",
                NATIVE_TOOLS.join(", ")
            ),
            "`medulla:shell` runs a script in the operator's project directory. \
             `args.script` is the source (required); `args.language` picks the interpreter — \
             `shell` (the default, run with bash), `javascript`, or `python`; `args.input` is \
             handed to it on stdin as JSON. It returns `{ output, stderr }`, where `output` is \
             the script's stdout parsed as JSON when it is JSON. Gated on \
             `workflows.allowCode` for the same reason `code` nodes are — check `workflow_host`."
                .to_string(),
            "Reach for `medulla:shell` before an `agent` node whenever the work is a \
             deterministic command. An `agent` node is a whole coding-harness session — minutes \
             of wall clock and a model deciding what to run — which is the right shape for work \
             needing judgement and a very expensive way to run `npm test`."
                .to_string(),
            "There is no third-party integration registry on this host, so an allowlisted \
             non-native slug will still fail at run time. Express host-specific steps as a \
             `medulla:` tool call or an `agent` node."
                .to_string(),
        ],
        "http_request" => vec![
            "Outbound HTTP is refused unless the target host is listed in \
             `workflows.httpAllowlist`. A bare domain also permits its subdomains."
                .to_string(),
            "Loopback, link-local, and private addresses are always refused, whatever the \
             allowlist says."
                .to_string(),
            "Credentials are never written into the graph. Set `config.connection_ref` to \
             `http_cred:<name>` and the host injects the header."
                .to_string(),
        ],
        "code" => vec![
            "Code nodes are disabled on this host by default: there is no sandbox, so workflow \
             code would run with the daemon's own privileges. `workflows.allowCode` enables \
             them, out-of-process and time-bounded — an explicit decision to trust whoever \
             wrote the workflow. Check `workflow_host` before authoring one."
                .to_string(),
            // The engine's own example is written for a host that wraps the
            // source in a function. Medulla executes the file as-is, so that
            // example is a syntax error here — stating the real convention is
            // the difference between a node that works and one that does not.
            "CALLING CONVENTION ON THIS HOST: the source is executed as a whole program, not \
             wrapped in a function. The input arrives on **stdin as JSON** (and as a file whose \
             path is `argv[1]` and `$MEDULLA_INPUT`); the result is whatever the program prints \
             to **stdout**, parsed as JSON when it is JSON and taken as a string otherwise. \
             `return` at the top level is a syntax error — print instead."
                .to_string(),
            "Working javascript: `const i = JSON.parse(require('fs').readFileSync(0, 'utf8')); \
             console.log(JSON.stringify({ total: i.a + i.b }));`. Working python: `import json, \
             sys; print(json.dumps({'total': sum(json.load(sys.stdin))}))`."
                .to_string(),
            "A `code` node runs in a scratch directory, not the operator's project — it is a \
             computation over its input. A step that means to touch the repository is a \
             `tool_call` with the `medulla:shell` slug, which runs in the workspace and says so \
             in the graph."
                .to_string(),
        ],
        "sub_workflow" => vec![
            "`workflow_id` resolves against the workflows this host has installed — the same \
             ids `medulla workflow list` prints."
                .to_string(),
        ],
        "trigger" => vec![
            "Only `manual` triggers fire on this host today: a run starts because an operator \
             or the orchestrator asked for it. Other kinds are accepted and stored, but \
             nothing dispatches them yet."
                .to_string(),
        ],
        _ => Vec::new(),
    };

    contract.notes.extend(notes);
    contract
}

/// Every node-kind contract, with host notes applied.
pub fn all_node_kind_contracts() -> Vec<NodeKindContract> {
    tinyflows::catalog::all_contracts()
        .into_iter()
        .map(apply_host_overlay)
        .collect()
}

/// One node-kind contract by kind, with host notes applied.
pub fn node_kind_contract(kind: &str) -> Option<NodeKindContract> {
    tinyflows::catalog::contract_for(kind).map(apply_host_overlay)
}

/// The node kinds as one line per kind, for embedding in a tool description.
///
/// Exists so an authoring tool's prose cannot drift from the catalogue: the
/// description is generated from the contracts rather than written beside them.
pub fn render_node_kinds_line() -> String {
    all_node_kind_contracts()
        .iter()
        .map(|contract| format!("{}: {}", contract.kind, contract.summary))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "node_contracts_tests.rs"]
mod tests;
