# Tests

Unit tests for the event vocabulary, split by the surface under test: `serde_tests` covers JSON round-trips and deserialize tolerance; `derive_tests` covers the read-only derivations.

## Contents

- [`derive_tests.rs`](./derive_tests.rs) — Tests for the read-only derivations: `chat_transcript`, `last_assistant_message`, and `describe_event`.
- [`mod.rs`](./mod.rs) — Unit tests for the event vocabulary, split by the surface under test: `serde_tests` covers JSON round-trips and deserialize tolerance; `derive_tests` covers the read-only derivations.
- [`serde_tests.rs`](./serde_tests.rs) — Tests for `TuiEvent` JSON serialization: full round-trips across every kind, and the deserialize tolerance rules (unknown kinds, missing fields, empty-string normalization, and error cases).

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
