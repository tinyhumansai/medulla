# Tests

Unit tests for contact-request admission, split by surface so no file exceeds the repo's 500-line ceiling: `policy` covers policy evaluation and the idempotent pending queue, `service` decision execution against a fake relay, and `health` what a poll reports about itself.

## Contents

- [`health.rs`](./health.rs) — What a poll reports about itself — and where it narrates.
- [`mod.rs`](./mod.rs) — Unit tests for contact-request admission, split by surface so no file exceeds the repo's 500-line ceiling: `policy` covers policy evaluation and the idempotent pending queue, `service` decision execution against a fake relay, and `health` what a poll reports about itself.
- [`policy.rs`](./policy.rs) — Policy evaluation and the idempotent pending queue.
- [`service.rs`](./service.rs) — Decision execution: what reaches the relay, and what it records.
- [`types.rs`](./types.rs) — Shared fixture types for contact admission tests.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
