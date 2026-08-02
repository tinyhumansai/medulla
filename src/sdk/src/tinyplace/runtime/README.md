# Runtime

Agent-runtime helpers layered on the tinyplace SDK client.

## Contents

- [`identity_pool/`](./identity_pool/) — Collision-free identity acquisition for the daemon.
- [`session_store/`](./session_store/) — Filesystem-backed `SessionStore` persistence for Signal ratchet/pre-key state, laid out to interoperate with the TS SDK's `FileSessionStore`.
- [`identity.rs`](./identity.rs) — Agent identity bootstrap: load-or-mint the 32-byte Ed25519 seed backing the tiny.place signer and persist it to the tinyplace CLI config file.
- [`mod.rs`](./mod.rs) — Agent-runtime helpers layered on the tinyplace SDK client.
- [`poll.rs`](./poll.rs) — Background poll loops layered on the tiny.place SDK client: destructive mailbox reads, fail-closed contact auto-acceptance, and presence heartbeats.
- [`tests.rs`](./tests.rs) — Unit tests for the tiny.place runtime helpers: identity bootstrap, the file-backed session store round-trips, and error mapping.
- [`types.rs`](./types.rs) — Data model for the tiny.place agent-runtime helpers.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
