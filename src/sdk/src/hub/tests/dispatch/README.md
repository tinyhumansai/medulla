# Dispatch

Tests for the sender-runner: dispatch, routing by `correlationId`, the ack window + reset/resend recovery, the no-progress watchdog, and orchestrator abort. Driven by the `FakeWorker` `Relay` harness, which replays a worker's frame sequence into the inbox with no network.

## Contents

- [`harness/`](./harness/) — The `FakeWorker` `Relay` the dispatch tests run against.
- [`mod.rs`](./mod.rs) — Tests for the sender-runner: dispatch, routing by `correlationId`, the ack window + reset/resend recovery, the no-progress watchdog, and orchestrator abort. Driven by the `FakeWorker` `Relay` harness, which replays a worker's frame sequence into the inbox with no network.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
