# Client

Thin Medulla surface over the shared `tinyhumans-sdk`.

The SDK owns essentially all of it: transport, credential headers, the
`{success, data}` envelope, path percent-encoding, the not-exposed-route gate,
the typed request and response models, and the SSE session stream. `types/` and
`program/` re-export those models under this crate's established names, so
callers here keep the names they already use while one definition of the
contract lives upstream.

What genuinely remains:

- **`ClientError`** — the taxonomy front ends branch on (`is_auth_error`),
  converted from `tinyhumans_sdk::Error` in `error/`. The conversion recovers an
  `errorCode` from a non-2xx body, without which a 401 carrying `TOKEN_EXPIRED`
  would stop reaching the login screen.
- **The history-reward calls** — this crate needs the full settled status and
  per-metric breakdown the reveal screen renders; the SDK exposes a narrower
  projection, so these decode through `raw()`.
- **`RunOptions`** — wraps the SDK's own `RunOptions` to keep
  `MedullaClient::run`'s signature stable.

Everything else is a one-line delegation.

## Contents

- [`error/`](./error/) — `ClientError` and the conversion from `tinyhumans_sdk::Error`.
- [`program/`](./program/) — Roster and task-program models, re-exported from the SDK.
- [`sse/`](./sse/) — Re-export of the SDK's SSE parser and event stream.
- [`tests/`](./tests/) — `decode_tests` covers event decoding and SDK error mapping; `sse_tests` covers the parser, dedupe cursor, and streaming; `integration_tests` covers the endpoint surface against a TCP stub.
- [`types/`](./types/) — SDK model re-exports plus the history-reward and run-option types this crate still owns.
- [`mod.rs`](./mod.rs) — The client: one method per endpoint.

## Maintenance

New endpoints belong on the SDK's typed namespace methods. If a response needs a
shape the SDK does not model, prefer widening the SDK over decoding it here —
this module going back to owning models is how three copies of the contract
appeared in the first place.

Reach for `raw()` only where this crate genuinely needs more than the SDK
exposes, and say so in a comment.
