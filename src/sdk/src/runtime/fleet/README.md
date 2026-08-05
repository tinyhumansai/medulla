# Fleet

The declared-capacity contracts: the strict single-parent containment chain `Host → Harness → Workspace → Agent`, the agent-template catalog that constrains what may be provisioned into it, and the `CapacitySnapshot` roll-up the UI renders.

## Contents

- [`declaration.rs`](./declaration.rs) — Operator-declared agents: the writable end of the fleet chain, and the source of this client's local roster.
- [`declaration_tests.rs`](./declaration_tests.rs) — Unit tests for agent declarations: the derivations the roster depends on, the migration seeding rule, and the config round-trip of the shape itself.
- [`demo.rs`](./demo.rs) — An env-gated stand-in fleet: enough declared capacity to exercise every fleet surface with no backend, no socket, and no registered peer.
- [`mod.rs`](./mod.rs) — The declared-capacity contracts: the strict single-parent containment chain `Host → Harness → Workspace → Agent`, the agent-template catalog that constrains what may be provisioned into it, and the `CapacitySnapshot` roll-up the UI renders.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
