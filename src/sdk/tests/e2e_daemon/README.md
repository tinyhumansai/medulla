# E2e Daemon

Focused test coverage for the E2e Daemon area and its immediate collaborators.

## Contents

- [`helpers.rs`](./helpers.rs) — Shared fixtures for the daemon e2e suites: a recording `send` sink, frame decoding helpers, a `DaemonConfig` builder, and injectable `run_task` runners (real-spawn, blocking, and model-recording). Re-exports the std/tokio/medulla types the grouped test modules need so they can rely on a single `use crate::helpers::*;`.
- [`injected.rs`](./injected.rs) — Daemon e2e over injected (deterministic, non-spawning) `run_task` runners: opencode input rejection, capacity + duplicate rejection, the `idle()` drain contract, and per-task model-hint resolution.
- [`spawn_path.rs`](./spawn_path.rs) — Daemon e2e over the REAL spawn path ([`run_provider_task`]) driving fake provider CLIs (shell scripts emitting realistic JSONL) via `TINYPLACE_*_BIN` overrides: task lifecycle, tool-status mapping, mid-run stdin forwarding, capabilities merge + digest fallback, plaintext DMs, and the idle watchdog.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
