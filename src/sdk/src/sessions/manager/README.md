# Manager

`SessionManager` — the operator-facing surface the Sessions tab drives, and the daemon-facing entry that runs a folded `TurnRequest`.

## Contents

- [`mod.rs`](./mod.rs) — `SessionManager` — the operator-facing surface the Sessions tab drives, and the daemon-facing entry that runs a folded `TurnRequest`.
- [`turns.rs`](./turns.rs) — Turn execution: the bounded/unbound split, the interactive and one-shot transports, and the binding capture that gives an unbound session continuity.
- [`types.rs`](./types.rs) — Data model for `SessionManager`: its configuration, the request to open a session, the per-session entry it stores, and the transcript line the UI renders.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
