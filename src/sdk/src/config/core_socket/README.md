# Core Socket

Core (`medulla-serve`) socket resolution and validation: where the unix socket path the core runtime attaches to comes from (`--core-socket` flag, `MEDULLA_CORE_SOCKET` env var, `[core]` config section, or the default runtime dir), and the fail-fast check that a resolved path is actually attachable *before* `CoreRuntime::attach` is handed a value it can only spin on. AGENTS.md treats socket paths as untrusted configuration to be validated at boundaries — this module is that boundary.

## Contents

- [`mod.rs`](./mod.rs) — Core (`medulla-serve`) socket resolution and validation: where the unix socket path the core runtime attaches to comes from (`--core-socket` flag, `MEDULLA_CORE_SOCKET` env var, `[core]` config section, or the default runtime dir), and the fail-fast check that a resolved path is actually attachable *before* `CoreRuntime::attach` is handed a value it can only spin on. AGENTS.md treats socket paths as untrusted configuration to be validated at boundaries — this module is that boundary.
- [`types.rs`](./types.rs) — Data types for the `core_socket` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
