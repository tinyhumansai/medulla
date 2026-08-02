# Tinyplace

tinyplace protocol + agent-runtime layer for the medulla TUI/daemon.

## Contents

- [`config/`](./config/) — CLI config-file model and endpoint resolution.
- [`consumer/`](./consumer/) — Receiver-side fold of the SDK's v2 harness stream.
- [`control/`](./control/) — Owner-to-machine control frames for the harness session bus.
- [`frames/`](./frames/) — The `medulla-tinyplace/1` task wire protocol.
- [`runtime/`](./runtime/) — Agent-runtime helpers layered on the tinyplace SDK client.
- [`screen/`](./screen/) — The `medulla.screen.v1` protocol: streaming a worker's live terminal to a watching orchestrator as synchronised *state* rather than a byte stream.
- [`service/`](./service/) — Background tiny.place presence service for the TUI process.
- [`status/`](./status/) — Derived session-status state machine for the SDK's v2 harness stream.
- [`system_info/`](./system_info/) — Cheap, local worker capacity discovery.
- [`env_tests.rs`](./env_tests.rs) — Tests for the env module.
- [`env.rs`](./env.rs) — Centralized environment-variable resolution for the harness wrapper and the headless daemon.
- [`mod.rs`](./mod.rs) — tinyplace protocol + agent-runtime layer for the medulla TUI/daemon.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
