# Events

The TUI event vocabulary: every library `CycleEvent` plus the host-sourced rows (cycle framing, conversation turns, agent/session status, effects). `TuiEvent` deserializes any JSON `{kind, ...}` shape, keeping unknown kinds as a passthrough so a newer backend never drops rows on an older TUI.

## Contents

- [`tests/`](./tests/) — Unit tests for the event vocabulary, split by the surface under test: `serde_tests` covers JSON round-trips and deserialize tolerance; `derive_tests` covers the read-only derivations.
- [`derive.rs`](./derive.rs) — Pure, read-only derivations over events: the `TuiEvent::kind` discriminator plus the transcript, last-message, and one-line-description helpers the UI layers build on. Nothing here mutates state or performs I/O.
- [`mod.rs`](./mod.rs) — The TUI event vocabulary: every library `CycleEvent` plus the host-sourced rows (cycle framing, conversation turns, agent/session status, effects). `TuiEvent` deserializes any JSON `{kind, ...}` shape, keeping unknown kinds as a passthrough so a newer backend never drops rows on an older TUI.
- [`serde_impl.rs`](./serde_impl.rs) — Custom `Serialize`/`Deserialize` for `TuiEvent`.
- [`types.rs`](./types.rs) — The event data model: the `TuiEvent` union and its payload structs plus the sequenced `EventEnvelope`. These are plain data types; the custom serialization lives in `super::serde_impl` and the read-only derivations in `super::derive`.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
