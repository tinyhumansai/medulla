# Caps

Assembling the capability bundle the engine runs against.

## Contents

- [`agent.rs`](./agent.rs) — `agent` nodes, run on a real harness.
- [`code.rs`](./code.rs) — The `code` node's runner — refusing by default.
- [`dispatch.rs`](./dispatch.rs) — Handing a workflow node's instruction to a harness.
- [`http.rs`](./http.rs) — Outbound HTTP for `http_request` nodes, behind a host allowlist.
- [`mocks.rs`](./mocks.rs) — Capability stand-ins for dry runs.
- [`mod.rs`](./mod.rs) — Assembling the capability bundle the engine runs against.
- [`state.rs`](./state.rs) — Durable key/value state for stateful workflows.
- [`tools.rs`](./tools.rs) — `tool_call` dispatch across two namespaces.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
