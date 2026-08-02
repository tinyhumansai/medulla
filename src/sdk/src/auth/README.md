# Auth

Login plumbing: an RFC 8252 loopback OAuth flow against the Medulla backend and the pure URL/query helpers the CLI and tests share.

## Contents

- [`loopback/`](./loopback/) — The RFC 8252 loopback OAuth flow: bind an ephemeral loopback port, classify and answer the browser callback, and capture the JWT. Holds the `LoopbackListener`, the `start_loopback`/`run_login_flow` entry points, the pure request classifier, and the browser opener.
- [`migrate.rs`](./migrate.rs) — Remove the retired Medulla-owned credential files.
- [`mod.rs`](./mod.rs) — Login plumbing: an RFC 8252 loopback OAuth flow against the Medulla backend and the pure URL/query helpers the CLI and tests share.
- [`tests.rs`](./tests.rs) — Unit tests for token resolution, the pure URL/query helpers, and the loopback request classifier.
- [`token.rs`](./token.rs) — Backend bearer-token resolution: pick the effective token from config, the environment, or the core's app session; describe the missing-token state; and classify one-time login tokens versus JWTs. Depends on `crate::config::BackendConfig` for the configured backend.
- [`types.rs`](./types.rs) — Plain data types for the auth module: the stored `Credentials`, the OAuth `Provider` enum, the loopback-flow `LoginError` and `LoopbackConfig`, and the `DEFAULT_LOGIN_TIMEOUT` constant. Behaviour-heavy types (the credential store, the loopback listener) live beside their logic.
- [`url.rs`](./url.rs) — Pure URL/query helpers shared by the loopback flow, the CLI, and tests: build the loopback redirect and backend login URLs, mint the state nonce, percent-encode/decode query values, parse a request target, and summarize the `/auth/me` response. No sockets or I/O — every function is a pure transformation over its inputs.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
