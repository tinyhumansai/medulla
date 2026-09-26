# Init

Workspace initialisation: registering a directory and authoring its `MEDULLA.md`.

## Contents

- [`tests/`](./tests/) — Unit tests for workspace initialisation. Every test is offline and deterministic; the profile body starts as an operator-editable stub.
- [`layout.rs`](./layout.rs) — Scanning a workspace's file layout for the orchestrator.
- [`MEDULLA.md.tmpl`](./MEDULLA.md.tmpl) — Provides the MEDULLA.md template emitted by SDK initialization.
- [`mod.rs`](./mod.rs) — Workspace initialisation: registering a directory and authoring its `MEDULLA.md`.
- [`registry.rs`](./registry.rs) — Enrolling an initialised directory in the operator's workspace registry.
- [`template.rs`](./template.rs) — Rendering a drafted profile into `MEDULLA.md` text.
- [`types.rs`](./types.rs) — Data types for workspace initialisation: the instruction files read from a directory, the operator-editable profile fields, and the outcome of a write.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
