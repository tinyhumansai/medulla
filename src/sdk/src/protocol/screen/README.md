# Screen

The `medulla.screen.v1` protocol: streaming a worker's live terminal to a watching orchestrator as synchronised *state* rather than a byte stream.

## Contents

- [`apply.rs`](./apply.rs) — Viewer side: fold an incoming frame into the screen held for a session.
- [`codec.rs`](./codec.rs) — Serializing screen messages into encrypted DM bodies, and recognising them on the way back.
- [`diff.rs`](./diff.rs) — Sender side: turn a pair of screens into the smallest frame that carries the newer one, and coalesce raw cells into the runs a frame is made of.
- [`mod.rs`](./mod.rs) — The `medulla.screen.v1` protocol: streaming a worker's live terminal to a watching orchestrator as synchronised *state* rather than a byte stream.
- [`tests.rs`](./tests.rs) — Unit tests for the screen protocol: coalescing, diffing, the viewer's fold, and the version-tagged envelope.
- [`types.rs`](./types.rs) — Data model for the `medulla.screen.v1` wire protocol: the synchronised terminal state, the frames that carry it, and their serde shapes.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
