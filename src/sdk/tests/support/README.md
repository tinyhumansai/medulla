# Support

Shared e2e test helpers: an in-test mock Medulla backend (HTTP + SSE) and fake provider-CLI scaffolding for the daemon runtime.

## Contents

- [`fake_provider.rs`](./fake_provider.rs) — Fake provider-CLI scaffolding for daemon e2e tests.
- [`mock_backend.rs`](./mock_backend.rs) — A minimal in-test mock of the Medulla backend HTTP + SSE API.
- [`mock_harness_helpers.rs`](./mock_harness_helpers.rs) — Canned scenarios, provider record builders, and temp-dir install helpers for the mock harness.
- [`mock_harness_relay.rs`](./mock_harness_relay.rs) — An in-memory tiny.place relay mock, just enough of the REST surface for the daemon's [`medulla::daemon::transport::SignalTransport`] to run encrypted DM round-trips with no network and no real backend.
- [`mock_harness_script.rs`](./mock_harness_script.rs) — Script rendering for [`MockCli`].
- [`mock_harness_types.rs`](./mock_harness_types.rs) — Core data types and the [`MockCli`] builder surface for the mock harness.
- [`mock_harness.rs`](./mock_harness.rs) — Mock coding-agent CLI harness for daemon e2e tests.
- [`mock_signal_server_http.rs`](./mock_signal_server_http.rs) — Wire-level HTTP + parsing helpers for the mock tiny.place Signal server.
- [`mock_signal_server_routing.rs`](./mock_signal_server_routing.rs) — Request routing for the mock tiny.place Signal server.
- [`mock_signal_server_state.rs`](./mock_signal_server_state.rs) — State model, fault-injection controls, and the server handle for the mock tiny.place Signal server.
- [`mock_signal_server.rs`](./mock_signal_server.rs) — A mock tiny.place **Signal server**: the server side of the end-to-end encrypted flows the vendored `tinyplace` SDK drives from the medulla runtime ([`medulla::daemon::transport::SignalTransport`], the wrapper bridge, and the `runtime` mailbox/contact/presence loops).
- [`mock_tinyplace.rs`](./mock_tinyplace.rs) — A minimal in-test mock of the tiny.place backend HTTP API.
- [`mod.rs`](./mod.rs) — Shared e2e test helpers: an in-test mock Medulla backend (HTTP + SSE) and fake provider-CLI scaffolding for the daemon runtime.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
