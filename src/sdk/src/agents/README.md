# Agents

Where agent templates come from: the built-in coding catalog, the on-disk `.medulla/agents/*.toml` store that supersedes it, and the installer that writes the catalog into that store so it can be edited.

## Contents

- [`defaults/`](./defaults/) — The built-in agent-template catalog: the coding roles every install knows without any files on disk.
- [`install.rs`](./install.rs) — Writing the built-in catalog into the store.
- [`mod.rs`](./mod.rs) — Where agent templates come from: the built-in coding catalog, the on-disk `.medulla/agents/*.toml` store that supersedes it, and the installer that writes the catalog into that store so it can be edited.
- [`store.rs`](./store.rs) — The on-disk template store: `.medulla/agents/*.toml`, one template per file.
- [`tests.rs`](./tests.rs) — Unit tests for the agent-template catalog and its on-disk store: the shape every default role must have, what the store does with good and bad files, and the install's never-overwrite rule.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
