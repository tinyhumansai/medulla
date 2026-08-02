# Tests

Unit tests for workspace initialisation. Every test is offline and deterministic; the profile body starts as an operator-editable stub.

## Contents

- [`layout.rs`](./layout.rs) — Unit tests for the workspace layout scan. Every test builds a real scratch tree — the scan's whole job is reading a filesystem, so faking that away would test nothing.
- [`mod.rs`](./mod.rs) — Unit tests for workspace initialisation and its deterministic profile scaffold.
- [`registry.rs`](./registry.rs) — Unit tests for workspace registration: what `medulla init` writes into the operator's config so the orchestrator can see and place a workspace.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
