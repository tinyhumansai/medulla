# Fixtures

Versioned, non-secret inputs used to keep integration tests deterministic and offline.

## Contents

- [`history/`](./history/) — Captured history samples used to verify session discovery, parsing, redaction, and upload behavior.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
