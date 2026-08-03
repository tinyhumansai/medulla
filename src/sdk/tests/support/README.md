# Support

Shared e2e test helpers: an in-test mock Medulla backend (HTTP + SSE) and fake provider-CLI scaffolding for the daemon runtime.

## Contents

- [`fake_provider.rs`](./fake_provider.rs) — Fake provider-CLI scaffolding for daemon e2e tests.
- [`mock_backend.rs`](./mock_backend.rs) — A minimal in-test mock of the Medulla backend HTTP + SSE API.
- [`mock_harness_helpers.rs`](./mock_harness_helpers.rs) — Canned scenarios, provider record builders, and temp-dir install helpers for the mock harness.
- [`mock_harness_script.rs`](./mock_harness_script.rs) — Script rendering for [`MockCli`].
- [`mock_harness_types.rs`](./mock_harness_types.rs) — Core data types and the [`MockCli`] builder surface for the mock harness.
- [`mock_harness.rs`](./mock_harness.rs) — Mock coding-agent CLI harness for daemon e2e tests.
- [`mod.rs`](./mod.rs) — Shared e2e test helpers: an in-test mock Medulla backend (HTTP + SSE) and fake provider-CLI scaffolding for the daemon runtime.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
