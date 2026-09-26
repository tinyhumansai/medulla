# Store

Where workflows and their run records live — Medulla's half of it. The store
itself (the `WorkflowStore` trait, the file-backed implementation, revisions,
the journal, proposal locks) moved to `tinyflows::store`, behind that crate's
`store` feature: none of that was ever about Medulla, and the sibling hosts
that embed the engine need exactly the same bookkeeping. Re-exported from here
so a call site still writes `crate::workflows::store::…`.

## Contents

- [`mod.rs`](./mod.rs) — The home layout (`workflow_dirs`, `workspace_state_dir`) and `MedullaPolicy`'s harness rule for a `defaults` block; everything else is re-exported from `tinyflows::store`.
- [`tests.rs`](./tests.rs) — Unit tests for what this crate contributes: the home layout and the harness-preference rule. The store's own behaviour (layered reads, atomic writes, revisions, the journal) is tested in `tinyflows::store`.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
