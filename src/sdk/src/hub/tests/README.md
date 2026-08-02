# Tests

Unit tests for the orchestrator hub, split by surface so no file exceeds the repo's 500-line ceiling: `activity` covers the in-memory activity ring and its attribution; `roster` covers advertising, addressing and dedupe; `dispatch` the sender-runner's full dispatch/route/settle path against a fake worker.

## Contents

- [`dispatch/`](./dispatch/) — Tests for the sender-runner: dispatch, routing by `correlationId`, the ack window + reset/resend recovery, the no-progress watchdog, and orchestrator abort. Driven by the `FakeWorker` `Relay` harness, which replays a worker's frame sequence into the inbox with no network.
- [`activity.rs`](./activity.rs) — Tests for the hub's `ActivityLog`: the in-memory record of what the hub's workers are doing right now.
- [`capabilities.rs`](./capabilities.rs) — Tests for the socket-plane capability advertisement: the hub probes a worker for its budgets/readiness over the encrypted relay and maps them onto the backend-shaped `capabilities` payload the backend's `sanitizeCapabilities` reads (`harnessBudgets`, `ready`, `readyReason`).
- [`mod.rs`](./mod.rs) — Unit tests for the orchestrator hub, split by surface so no file exceeds the repo's 500-line ceiling: `activity` covers the in-memory activity ring and its attribution; `roster` covers advertising, addressing and dedupe; `dispatch` the sender-runner's full dispatch/route/settle path against a fake worker.
- [`roster.rs`](./roster.rs) — Tests for the hub roster: how a worker is advertised, addressed, and kept unique.
- [`system_info.rs`](./system_info.rs) — Tests for lightweight worker capacity probes over the encrypted relay.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
