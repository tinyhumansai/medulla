# Task Loop

The frame- and task-handling half of `DaemonRuntime`, split by what each kind of frame asks for so no file exceeds the repo's 500-line ceiling: `probe` answers the cached capability probe, `system_info` reports cheap host capacity, `control` delivers mid-run input and stops a task the requester has given up on, and `run` executes a task with its slot limit, throttled status forwarding, and plain-text fallback.

## Contents

- [`control.rs`](./control.rs) — Mid-run input, and stopping a task the requester has given up on.
- [`mod.rs`](./mod.rs) — The frame- and task-handling half of `DaemonRuntime`, split by what each kind of frame asks for so no file exceeds the repo's 500-line ceiling: `probe` answers the cached capability probe, `system_info` reports cheap host capacity, `control` delivers mid-run input and stops a task the requester has given up on, and `run` executes a task with its slot limit, throttled status forwarding, and plain-text fallback.
- [`probe.rs`](./probe.rs) — Answering a peer's capability probe from the cached snapshot.
- [`run.rs`](./run.rs) — Executing one delegated task: slots, status forwarding, fallback.
- [`system_info.rs`](./system_info.rs) — Answering a routing hub's lightweight worker system-information request.
- [`workflow.rs`](./workflow.rs) — Running an installed workflow in answer to a task frame.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
