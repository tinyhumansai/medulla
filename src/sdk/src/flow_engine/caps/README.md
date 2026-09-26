# Caps

Assembling the capability bundle the engine runs against.

## Contents

- [`agent.rs`](./agent.rs) — `agent` nodes, run on a real harness.
- [`dispatch.rs`](./dispatch.rs) — Handing a workflow node's instruction to a harness.
- [`mod.rs`](./mod.rs) — Assembling the capability bundle the engine runs against.
- [`tools.rs`](./tools.rs) — `tool_call` dispatch across two namespaces.

## What is no longer here

The capability implementations with no Medulla in them — the out-of-process
script runner and its path policy, the `code` and `shell` runners, the file
state store, the allowlisted HTTP client, and the dry-run stand-ins — moved to
`tinyflows::caps::host`, behind that crate's `host-caps` feature. Every host
embedding the engine needs them, and each one that wrote them itself rewrote the
same subtle parts. [`mod.rs`](./mod.rs) re-exports them, so a call site in this
crate still names one place.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
