# Examples

Runnable examples that demonstrate narrow SDK contracts without becoming production entry points.

## Contents

- [`coordination_owner.rs`](./coordination_owner.rs) — The owner side of the coordination e2e chain: create a fresh owner identity, publish Signal pre-keys, send a `medulla-tinyplace/1` task frame to the worker daemon over the mock Signal server, then drain the encrypted mailbox until a terminal (reply/error) frame comes back — decrypt it and print it as JSON on stdout for `run.sh` to assert the mock-LLM marker on.
- [`harness_contract_decode.rs`](./harness_contract_decode.rs) — Decode one `TrackedTask` JSON value through medulla's Rust harness mirror.
- [`mock_signal_server.rs`](./mock_signal_server.rs) — A runnable wrapper around the in-test mock tiny.place **Signal server** (`tests/support/mock_signal_server.rs`), so the coordination e2e suite can run the exact same server as a standalone process the `medulla daemon` and the owner helper talk to over loopback.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
