# Core Host

Boot the embedded OpenHuman core in this process.

## Contents

- [`auth_tests.rs`](./auth_tests.rs) — Unit tests for the auth helpers' pure parts.
- [`auth.rs`](./auth.rs) — Sign a booted core in, so its Medulla surface has a session to use.
- [`mod.rs`](./mod.rs) — Boot the embedded OpenHuman core in this process.
- [`tests.rs`](./tests.rs) — Unit tests for workspace/action-dir derivation.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
