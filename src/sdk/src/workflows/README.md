# Workflows

Authored, durable, multi-step work: workflow definitions and their runs.

## Contents

- [`copilot/`](./copilot/) — The workflow copilot: a harness turn scoped to one graph.
- [`mcp/`](./mcp/) — A Model Context Protocol server exposing the workflow operations.
- [`run/`](./run/) — Running a workflow, and resuming one that paused.
- [`skills/`](./skills/) — Harness-native skills that trigger saved workflows over MCP.
- [`store/`](./store/) — Medulla's half of the store: the home layout, and the harness rule for a `defaults` block. The store itself is `tinyflows::store`.
- [`authoring_tests.rs`](./authoring_tests.rs) — The one patch-editing test that needs this host's own operations.
- [`dispatch_error.rs`](./dispatch_error.rs) — Turning a hub dispatch failure into a workflow failure.
- [`local.rs`](./local.rs) — Running workflows on this machine, with no orchestrator involved.
- [`mod.rs`](./mod.rs) — Authored, durable, multi-step work: workflow definitions and their runs.
- [`node_contracts_tests.rs`](./node_contracts_tests.rs) — Tests for the host overlay on the node-kind catalogue.
- [`node_contracts.rs`](./node_contracts.rs) — The node-kind catalogue, with this host's facts layered on.
- [`ops_tests.rs`](./ops_tests.rs) — Tests for the shared operation surface.
- [`ops.rs`](./ops.rs) — The workflow operations, as one JSON-in/JSON-out surface.
- [`registry.rs`](./registry.rs) — Resolving a workflow id to a graph for the engine.
- [`tests.rs`](./tests.rs) — Unit tests for the workflow record's derived views and for resolving sub-workflows out of a store.

## What is no longer here

The stored model (`types/`) and the file-backed store (`store/file/`) moved to
`tinyflows::store`, behind that crate's `store` feature: a workflow document is
the engine's own graph plus bookkeeping, and every host embedding the engine
needs the same bookkeeping. So did the run diagnosis (`run/diagnose.rs`) and the
expression-binding reader (`gates/bindings.rs`), patch-based editing
(`authoring.rs`), and the host-agnostic authoring gates. All are re-exported
from here, so a call site still writes `crate::workflows::WorkflowRecord` and
`crate::workflows::authoring::apply_workflow_ops`.

What stays is what needs this host's vocabulary: `gates/harness.rs`, and
`gates::MedullaPolicy`, which is how a store applies both the engine's rules and
this host's to every document it loads and every edit it writes.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
