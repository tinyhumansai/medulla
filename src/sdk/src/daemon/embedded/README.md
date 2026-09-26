# Embedded

The embedded host: a `DaemonRuntime` driven over any `Bridge`, inside someone else's process.

## Contents

- [`mod.rs`](./mod.rs) — The embedded host: a `DaemonRuntime` driven over any `Bridge`, inside someone else's process.
- [`tests.rs`](./tests.rs) — Unit tests for the embedded host: start-up validation, device-local task round-trips, and the observation counters a UI renders.
- [`types.rs`](./types.rs) — Data types for the embedded daemon: its start-up options, its live observation snapshot, and the handle a host keeps it alive by.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
