# Frames

The `medulla-task/1` task wire protocol.

## Contents

- [`decode.rs`](./decode.rs) — Task-frame parsing: recover a `TaskFrame` from a decrypted body and an `AgentCapabilities` object from a `capabilities_result` payload. Both are tolerant of foreign or malformed input and never panic.
- [`encode.rs`](./encode.rs) — Task-frame construction: turn an `EncodeFrameInput` into a serialized `medulla-task/1` frame body ready for an encrypted message.
- [`mod.rs`](./mod.rs) — The `medulla-task/1` task wire protocol.
- [`tests.rs`](./tests.rs) — Unit tests for the `medulla-task/1` frame codec: encode/decode round-trips, optional-field handling, and tolerant capabilities parsing.
- [`types.rs`](./types.rs) — Data model for the `medulla-task/1` task wire protocol: the frame structs and enums, their trivial serde/inherent `impl`s, and the tolerant `deserialize_with` helpers the `AgentCapabilities` derive depends on. The construction and parsing logic lives in the sibling `encode` and `decode` modules.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
