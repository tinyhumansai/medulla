# Examples

Runnable examples that demonstrate narrow SDK contracts without becoming production entry points.

## Contents

- [`harness_contract_decode.rs`](./harness_contract_decode.rs) — Decode one `TrackedTask` JSON value through medulla's Rust harness mirror.
- [`mock_link_forwarder.rs`](./mock_link_forwarder.rs) — A blind loopback UDP forwarder implementing `docs/host-link-protocol.md` §5, standing in for the backend in the coordination e2e harness.
- [`coordination_owner.rs`](./coordination_owner.rs) — The orchestrator end of that harness: enrolls a pair, then dispatches task frames over the host link and prints the terminal frame as JSON.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
