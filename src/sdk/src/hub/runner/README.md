# Runner

The bridge-independent task sender — the outbound half of the harness plane.

## Contents

- [`capabilities.rs`](./capabilities.rs) — Lightweight worker capability probes.
- [`mod.rs`](./mod.rs) — The bridge-independent task sender — the outbound half of the harness plane.
- [`pump.rs`](./pump.rs) — The inbox pump — the inbound half of the runner.
- [`system_info.rs`](./system_info.rs) — Lightweight worker system-information requests.
- [`types.rs`](./types.rs) — Data types for hub task dispatch and reply correlation.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
