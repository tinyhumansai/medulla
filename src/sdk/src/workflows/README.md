# Workflows

Authored, durable, multi-step work: workflow definitions and their runs.

## Contents

- [`copilot/`](./copilot/) — The workflow copilot: a harness turn scoped to one graph.
- [`mcp/`](./mcp/) — A Model Context Protocol server exposing the workflow operations.
- [`run/`](./run/) — Running a workflow, and resuming one that paused.
- [`store/`](./store/) — Where workflows and their run records live.
- [`authoring_tests.rs`](./authoring_tests.rs) — Tests for patch-based workflow editing.
- [`authoring.rs`](./authoring.rs) — Editing a workflow as a series of patches.
- [`local.rs`](./local.rs) — Running workflows on this machine, with no orchestrator involved.
- [`mod.rs`](./mod.rs) — Authored, durable, multi-step work: workflow definitions and their runs.
- [`node_contracts_tests.rs`](./node_contracts_tests.rs) — Tests for the host overlay on the node-kind catalogue.
- [`node_contracts.rs`](./node_contracts.rs) — The node-kind catalogue, with this host's facts layered on.
- [`ops_tests.rs`](./ops_tests.rs) — Tests for the shared operation surface.
- [`ops.rs`](./ops.rs) — The workflow operations, as one JSON-in/JSON-out surface.
- [`registry.rs`](./registry.rs) — Resolving a workflow id to a graph for the engine.
- [`tests.rs`](./tests.rs) — Unit tests for the workflow record's derived views and for resolving sub-workflows out of a store.
- [`types.rs`](./types.rs) — The data model for stored workflows and their runs.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
