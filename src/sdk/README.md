# SDK

The `medulla` library crate: reusable clients, runtimes, orchestration, persistence, tiny.place integration, and UI-facing state shared by Medulla applications.

## Contents

- [`examples/`](./examples/) — Runnable examples that demonstrate narrow SDK contracts without becoming production entry points.
- [`src/`](./src/) — The Rust module tree for the `medulla` SDK crate. `lib.rs` defines the public surface; child folders separate transport, runtime, orchestration, integration, persistence, and UI-facing responsibilities.
- [`tests/`](./tests/) — Cross-module integration, feature, and mocked end-to-end coverage for the SDK.
- [`Cargo.toml`](./Cargo.toml) — Declares the SDK crate, its dependencies, examples, and integration-test targets.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
