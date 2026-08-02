# Bridge

Message delivery bridges for local and remote agent communication.

## Contents

- [`local.rs`](./local.rs) — In-memory bridge for agents running within the same process.
- [`mod.rs`](./mod.rs) — Message delivery bridges for local and remote agent communication.
- [`routing_tests.rs`](./routing_tests.rs) — Unit tests for `RoutingBridge`: which side of the router an address lands on, and that the local side never reaches the remote transport.
- [`routing.rs`](./routing.rs) — Address-routing bridge: one endpoint that speaks to device-local peers over the in-memory bus and to everyone else over tiny.place.
- [`tests.rs`](./tests.rs) — Contract tests for bridge selection and device-local message delivery.
- [`tinyplace.rs`](./tinyplace.rs) — Remote bridge backed by tiny.place's encrypted Signal transport.
- [`types.rs`](./types.rs) — Shared bridge contract, kind discriminator, and runtime-selected transport.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
