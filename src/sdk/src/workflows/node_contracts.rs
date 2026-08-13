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
            format!(
                "`config.harness` chooses *what* runs the step, as distinct from `agent_ref`, \
                 which chooses *where*: one of {}, or the id of a custom harness preset the \
                 worker exposes. `config.model` is the model hint. Omit both to inherit the \
                 workflow's `defaults` block, and then the host's configuration.",
                crate::flow_engine::HarnessSelector::builtin_names().join(", ")
            ),
            "Naming a `harness` and no `model` runs that harness on its own default model — an \
             inherited model is deliberately left behind, because a model id chosen for one \
             harness is meaningless or wrong on another. Name both when you mean both."
                .to_string(),
            "`harness` must be written plainly, never as a `=`-expression: which binary and \
             which credentials run a step is a decision the graph makes, not one its data makes. \
             Authoring one is refused rather than saved. `model` may be an expression."
                .to_string(),
            "The harness reply is available as `=item.text`. If the harness replied with JSON \
             it is also parsed, so `=item.json.<field>` works without an output_parser."
                .to_string(),
            "`execution: \"per_item\"` with a `concurrency` is how you run several harnesses at \
             once — one session per input item, so `split_out` → agent(concurrency: 4) → merge \
             works through a list in parallel and hands the merge one result per item. Bear in \
             mind each item is a whole coding session, not a model call: fan out over a list you \
             have already narrowed."
                .to_string(),
            "This host caps how many harness tasks a run may have in flight (`workflows.\
             maxParallelAgents`, 4 by default) because a dispatch occupies a worker for the \
             length of a coding task. A higher `concurrency` is throttled to that ceiling, never \
             refused — check `workflow_host` for the current value."
                .to_string(),
        ],
        "tool_call" => vec![
            format!(
                "Slugs beginning `{NATIVE_TOOL_PREFIX}` are built into this host: {}. Any other \
                 slug must be listed in `workflows.toolAllowlist`, and is refused otherwise.",
                NATIVE_TOOLS.join(", ")
            ),
            "`medulla:shell` runs a script in the operator's project directory. `args.script` \
             is an inline script and `args.script_path` runs a file the repository already has \
             — exactly one of the two, never both. `args.language` picks the interpreter — \
             `shell` (the default, run with bash), `javascript`, or `python`; `args.input` is \
             handed to it on stdin as JSON. It returns `{ output, stderr }`, where `output` is \
             the script's stdout parsed as JSON when it is JSON. Gated on \
             `workflows.allowCode` for the same reason `code` nodes are — check `workflow_host`."
                .to_string(),
            "`args.cwd` narrows where a step runs (the workspace root by default) and \
             `args.env` adds variables to the inherited environment, as an object of strings. \
             Reach for `script_path` when the script is something the repository maintains: a \
             copy pasted into the graph drifts from the one that is actually kept up to date."
                .to_string(),
            "`args.script_path` and `args.cwd` are resolved inside the workspace and refused \
             anywhere else — relative only, no `..`, and re-checked after symlinks are followed. \
             A host with no configured workspace refuses both, and `args.script` is the way in."
                .to_string(),
            "The workspace itself is chosen per run, by `workflow_run`'s `workspace` parameter, \
             and every step reads it from `$MEDULLA_WORKSPACE`. Do not declare an input for the \
             checkout a workflow works on: a step's `cwd` cannot leave the workspace, so a path \
             passed that way can only ever name somewhere already inside it. Write the workflow \
             as if it were standing in the repository, and let the caller say which one."
                .to_string(),
            "`language: shell` requires a unix host: there is no portable POSIX shell on \
             Windows, and this host refuses rather than emulating one, so a `shell` step run \
             there fails outright. `javascript` and `python` need no such shell and work on any \
             host — prefer them when the workflow may run on Windows. `workflow_host` reports \
             whether the current host can run shell scripts."
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
            "Code nodes are enabled by default and there is no sandbox, so workflow code runs \
             with the daemon's own privileges. Set `workflows.allowCode = false` when definitions \
             are not trusted; enabled code still runs out-of-process and time-bounded. Check \
             `workflow_host` before authoring one."
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
            "`language` must be exactly `javascript` or `python`. The engine matches those two \
             literal strings and silently treats everything else — `python3`, `js`, `shell` — as \
             JavaScript, so a near-miss spelling runs your program through node and fails with a \
             syntax error naming an interpreter you did not choose. Authoring one is refused \
             rather than saved. A `code` node cannot run shell at all: that is `medulla:shell`."
                .to_string(),
            "THE INPUT IS AN ARRAY OF ITEMS, not the upstream value directly: \
             `[{\"json\": <value>, \"paired_item\": 0}, …]`. Reach the first item's value with \
             `items[0].json`. When the upstream node is an `agent`, `tool_call`, or \
             `http_request`, that value is itself the `{json, text, raw}` envelope — so a field \
             from a `medulla:shell` step upstream is `items[0].json.json.output.<field>`. \
             Getting this wrong is the most likely way a working script still fails."
                .to_string(),
            "Working javascript: `const items = JSON.parse(require('fs').readFileSync(0, \
             'utf8')); console.log(JSON.stringify({ total: items[0].json.a + items[0].json.b \
             }));`. Working python: `import json, sys; items = json.load(sys.stdin); \
             print(json.dumps({'total': items[0]['json']['a']}))`."
                .to_string(),
            "A `code` node runs in a scratch directory, not the operator's project — it is a \
             computation over its input. A step that means to touch the repository is a \
             `tool_call` with the `medulla:shell` slug, which runs in the workspace and says so \
             in the graph."
                .to_string(),
            "A dry run does NOT execute a code node: the simulation mocks the runner, so a \
             script with a syntax error or the wrong output shape passes one cleanly. \
             `workflow_run` is the only thing that proves a script works."
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
        "loop" => vec![
            "A pass through the body costs what the body costs, and on this host an `agent` \
             node in the body is a whole coding-harness session. A loop of 10 over an agent \
             node is ten coding sessions and can run for an hour — keep `max_iterations` as \
             small as the job allows, and prefer a `condition` so the loop stops when the work \
             is done rather than always running to the cap."
                .to_string(),
            "The run's wall clock still applies on top of the cap: a loop that would need more \
             than `workflows.runTimeoutSecs` (600 by default) is cut off there regardless of \
             how many iterations remain. Check `workflow_host` for the current value."
                .to_string(),
            "Three shapes are refused rather than silently running once or never: a fan-in \
             `merge` node on the cycle (a single-input `merge` is a passthrough and is fine), \
             a loop node that is itself a fan-in, and a `body` that does not route back to the \
             loop head. For the fan-in cases, join the incoming arms with a `merge` placed \
             *before* the loop node, outside the cycle; for the last, wire `body` to the first \
             node of the loop body and wire that body's last node back to this loop."
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
