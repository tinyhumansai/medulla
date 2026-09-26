# Fleet

Pure view-model for the fleet: project the locally registered peers onto the declared containment chain (`Host → Harness → Workspace`) and merge that with whatever the runtime itself declares.

## Contents

- [`mod.rs`](./mod.rs) — Pure view-model for the fleet: project the locally registered peers onto the declared containment chain and merge that with whatever the runtime declares.
- [`registry.rs`](./registry.rs) — Project the local peer registry onto the containment chain.
- [`tests.rs`](./tests.rs) — Unit tests for the fleet view-model: the registry projection, the merge, and the env-gated stand-in fleet.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
