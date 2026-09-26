# Mock

A scripted, self-contained `Runtime` used by `main` until the backend runtime lands, and by tests. It fabricates a plausible event stream so every tab has something to render.

## Contents

- [`mod.rs`](./mod.rs) — A scripted, self-contained `Runtime` used by `main` until the backend runtime lands, and by tests. It fabricates a plausible event stream so every tab has something to render.
- [`runtime_impl.rs`](./runtime_impl.rs) — The `Runtime` trait implementation for `MockRuntime`: snapshotting, subscription, the scripted submit/abort/session lifecycle, forking, and the memory-surface reads. This is the behaviour half of the mock; the data model it drives lives in `super::types`.
- [`scenario.rs`](./scenario.rs) — The populated demo scenario: scripts a plausible roster, presence, a couple of chat turns, and a completed delegated task so every tab has something to render. Kept apart from `super::types` because it is scenario data rather than reusable structure.
- [`tests.rs`](./tests.rs) — Unit tests for the scripted mock runtime: demo population, thread forking, the submit/abort/session lifecycle, the memory surface, and change notifications.
- [`types.rs`](./types.rs) — Data model and trivial construction/mutation seams for the scripted mock runtime.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
