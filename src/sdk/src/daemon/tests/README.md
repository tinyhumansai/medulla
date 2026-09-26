# Tests

Runtime state-machine tests driven by a fake executor (no network, no CLIs), split by surface: `task_tests` covers task acceptance, dispatch, input forwarding, and shutdown; `provider_tests` covers provider selection and plain-text DM routing; `capability_tests` covers the cached capability probe, status throttling, and semantic-event → status-line mapping.

## Contents

- [`capability_tests.rs`](./capability_tests.rs) — Capability-probe caching, status-frame throttling, and the pure semantic-event → status-line mapping (`status_detail`).
- [`mod.rs`](./mod.rs) — Runtime state-machine tests driven by a fake executor (no network, no CLIs), split by surface: `task_tests` covers task acceptance, dispatch, input forwarding, and shutdown; `provider_tests` covers provider selection and plain-text DM routing; `capability_tests` covers the cached capability probe, status throttling, and semantic-event → status-line mapping.
- [`provider_tests.rs`](./provider_tests.rs) — Provider-selection and plain-text DM routing tests: requesting an unavailable provider, falling back when the default is absent, running the default provider for a raw DM, and refusing plain text at capacity or with no provider offered.
- [`system_info_tests.rs`](./system_info_tests.rs) — Tests for daemon-side lightweight system-information responses.
- [`task_tests.rs`](./task_tests.rs) — Task-lifecycle tests: acceptance limits, duplicate rejection, stdin/input forwarding (including buffering before the sink registers), and shutdown aborting an in-flight task.
- [`types.rs`](./types.rs) — Shared fixture types for daemon runtime tests.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
