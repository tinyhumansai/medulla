# Loopback

The RFC 8252 loopback OAuth flow: bind an ephemeral loopback port, classify and answer the browser callback, and capture the JWT. Holds the `LoopbackListener`, the `start_loopback`/`run_login_flow` entry points, the pure request classifier, and the browser opener.

## Contents

- [`mod.rs`](./mod.rs) — The RFC 8252 loopback OAuth flow: bind an ephemeral loopback port, classify and answer the browser callback, and capture the JWT. Holds the `LoopbackListener`, the `start_loopback`/`run_login_flow` entry points, the pure request classifier, and the browser opener.
- [`types.rs`](./types.rs) — Data types for the `loopback` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
