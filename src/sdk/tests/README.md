# Tests

Cross-module integration, feature, and mocked end-to-end coverage for the SDK.

## Contents

- [`e2e_daemon/`](./e2e_daemon/) — Focused test coverage for the E2e Daemon area and its immediate collaborators.
- [`fixtures/`](./fixtures/) — Versioned, non-secret inputs used to keep integration tests deterministic and offline.
- [`support/`](./support/) — Shared e2e test helpers: an in-test mock Medulla backend (HTTP + SSE) and fake provider-CLI scaffolding for the daemon runtime.
- [`e2e_daemon_providers.rs`](./e2e_daemon_providers.rs) — (Unix-only: exercises Unix-domain-socket cores and/or spawned `/bin/sh` mock scripts.)
- [`e2e_daemon_router.rs`](./e2e_daemon_router.rs) — (Unix-only: exercises spawned `/bin/sh` fake-provider scripts.)
- [`e2e_daemon.rs`](./e2e_daemon.rs) — (Unix-only: exercises Unix-domain-socket cores and/or spawned `/bin/sh` mock scripts.)
- [`e2e_history_rewards.rs`](./e2e_history_rewards.rs) — Mocked end-to-end coverage for the history-reward client methods.
- [`e2e_local_bridge.rs`](./e2e_local_bridge.rs) — End-to-end task dispatch over the device-local bridge with no remote transport, identity, or network server.
- [`e2e_local_host.rs`](./e2e_local_host.rs) — End-to-end: the orchestrator half and the host half in one process.
- [`e2e_update.rs`](./e2e_update.rs) — End-to-end tests for the release update checker and self-updater ([`medulla::update`]) against a hand-rolled stub HTTP server (the same TcpListener pattern the other e2e mocks use). No real network, no GitHub.
- [`feature_history_upload.rs`](./feature_history_upload.rs) — Feature tests for history sharing against realistic transcripts.
- [`feature_init.rs`](./feature_init.rs) — End-to-end workspace-initialisation tests: the full `medulla init` flow over a real directory tree, from reading instruction files through drafting, writing, reading back, and building the run-request payload.
- [`feature_status.rs`](./feature_status.rs) — Coverage for the derived session-status machine branches that the inline `protocol::status` tests do not reach: agent thinking/message derivations, the lifecycle phase ladder, and the empty-call-id path.
- [`feature_workflow_dispatch.rs`](./feature_workflow_dispatch.rs) — An orchestrator dispatching a *workflow* to a worker, end to end.
- [`feature_workflow_examples.rs`](./feature_workflow_examples.rs) — The shipped example workflows must stay valid.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
