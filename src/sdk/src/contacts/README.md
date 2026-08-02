# Contacts

Incoming contact-request management for tiny.place peers.

## Contents

- [`book/`](./book/) — `ContactBook` — the pending-request queue the operator works through, and the policy that decides which requests never reach them.
- [`desk/`](./desk/) — `ContactDesk` — the book, the relay, and the clock bundled into the one handle a UI holds.
- [`service/`](./service/) — The relay side of contact management: poll incoming requests into a `ContactBook`, apply the admission policy, and perform operator decisions.
- [`tests/`](./tests/) — Unit tests for contact-request admission, split by surface so no file exceeds the repo's 500-line ceiling: `policy` covers policy evaluation and the idempotent pending queue, `service` decision execution against a fake relay, and `health` what a poll reports about itself.
- [`mod.rs`](./mod.rs) — Incoming contact-request management for tiny.place peers.
- [`types.rs`](./types.rs) — Data model for incoming contact-request management: the admission policy, a pending request, and the operator's decision.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
