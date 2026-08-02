# Hub

The task-sender hub — the outbound half of the harness plane.

## Contents

- [`activity/`](./activity/) — What the hub's workers are actually doing, recorded as it happens.
- [`boot/`](./boot/) — Hub bootstrap: construct the remote tiny.place bridge + sender-runner, connect the Socket.IO harness client, and expose a live `HubHandle`.
- [`handle/`](./handle/) — The live control handle over the hub's worker roster.
- [`pairing/`](./pairing/) — Inbound pairing for the hub's own tiny.place identity.
- [`probe/`](./probe/) — Building the `capabilities` payload the backend records for a worker.
- [`roster/`](./roster/) — The hub's worker-roster data: the shared roster type, the `AgentDescriptor` payload the hub advertises, and the address resolution the socket layer uses to target a task. Pure and offline-testable; the live control handle that mutates the roster over the Socket.IO uplink lives in `handle`.
- [`runner/`](./runner/) — The bridge-independent task sender — the outbound half of the harness plane.
- [`screens/`](./screens/) — What the hub currently sees of its workers' screens.
- [`tests/`](./tests/) — Unit tests for the orchestrator hub, split by surface so no file exceeds the repo's 500-line ceiling: `activity` covers the in-memory activity ring and its attribution; `roster` covers advertising, addressing and dedupe; `dispatch` the sender-runner's full dispatch/route/settle path against a fake worker.
- [`mod.rs`](./mod.rs) — The task-sender hub — the outbound half of the harness plane.
- [`relay.rs`](./relay.rs) — Compatibility name for the hub's shared local-or-remote bridge contract.
- [`socket.rs`](./socket.rs) — The Socket.IO harness client — the hub's uplink to the hosted backend brain.
- [`types.rs`](./types.rs) — Data types for the bridge-independent task sender: a dispatch request, its terminal outcome, and the error a dispatch can fail with.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
