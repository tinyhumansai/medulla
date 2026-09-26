# Tests

Unit and integration tests for the Medulla client, split by surface: `decode_tests` covers envelope/error/run-result JSON decoding; `sse_tests` covers the SSE parser, dedupe cursor, and streaming; `integration_tests` covers the HTTP endpoint surface against a TCP stub.

## Contents

- [`decode_tests.rs`](./decode_tests.rs) — Decode fixtures: event envelope/kind decoding, success/error envelope unwrapping and error mapping, and `LoopEvent`/`RunResult` deserialization.
- [`integration_tests.rs`](./integration_tests.rs) — End-to-end tests of the HTTP endpoint surface driven against a local TCP stub, asserting on request lines/bodies and the decoded responses as well as transport/decode error paths.
- [`mod.rs`](./mod.rs) — Unit and integration tests for the Medulla client, split by surface: `decode_tests` covers envelope/error/run-result JSON decoding; `sse_tests` covers the SSE parser, dedupe cursor, and streaming; `integration_tests` covers the HTTP endpoint surface against a TCP stub.
- [`sse_tests.rs`](./sse_tests.rs) — SSE-focused tests: the incremental frame parser, the reconnect dedupe cursor, and end-to-end streaming (including decode/empty/connect edges) driven through the shared TCP stub.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
