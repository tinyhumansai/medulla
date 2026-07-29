# Client

Typed Medulla surface over the shared `tinyhumans-sdk` transport.

Requests are issued by `tinyhumans_sdk::TinyHumansClient`, which owns credential
headers, the `{success, data}` envelope, path percent-encoding, and the
not-exposed-route gate. This module is not a second HTTP client — it adds what
the shared SDK does not model:

- **Typed DTOs.** The SDK returns open `DynamicResponse` JSON for these routes;
  the models in `types/` and `program/` give them a checked shape.
- **The `ClientError` taxonomy.** Front ends branch on one predicate
  (`is_auth_error`), so the SDK's `Status`/`Envelope` split collapses into
  `ClientError::Api` with the `errorCode` recovered from either.
- **The SSE event stream.** The SDK's transport buffers a whole response body,
  which a stream that never ends cannot use.

A few routes use `TinyHumansClient::raw`, the SDK's own escape hatch, where this
crate models a contract the SDK's request types cannot express — a tri-state
recurrence patch, the `sync=1`/`sync=0` message flag, the NDJSON transcript
upload. Each says why inline. They still share the one transport.

## Contents

- [`error/`](./error/) — Error type for the Medulla client, and the conversion from `tinyhumans_sdk::Error`.
- [`program/`](./program/) — Typed models shared by the public worker-roster and task-program endpoints.
- [`sse/`](./sse/) — Hand-rolled Server-Sent Events parsing and a reconnecting event stream.
- [`tests/`](./tests/) — Unit and integration tests for the Medulla client, split by surface: `decode_tests` covers event/run-result JSON decoding and SDK error mapping; `sse_tests` covers the SSE parser, dedupe cursor, and streaming; `integration_tests` covers the HTTP endpoint surface against a TCP stub.
- [`types/`](./types/) — JSON types mirroring the backend API responses.
- [`mod.rs`](./mod.rs) — The client itself: one method per endpoint, over the SDK.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data
structures in `types.rs`, focused unit tests in `tests.rs` or a sibling
`_tests.rs`, and preserve the module-level Rust documentation as the API source
of truth.

New endpoints should prefer the SDK's typed namespace methods. Reach for `raw()`
only when this crate's contract is genuinely finer than the SDK's request type,
and say so in a comment — otherwise the shared transport quietly stops being the
one place the backend contract lives.
