# Tests

Unit tests for the session model, split by surface so no file exceeds the repo's 500-line ceiling: `routing_tests` covers class/transport routing and the provider capability matrix; `registry_tests` the binding registry and turn serialization; `input_tests` the task-frame/envelope driver seam; `completion_tests` interactive turn-completion detection; and `manager_tests` the session lifecycle and turn execution.

## Contents

- [`manager_tests/`](./manager_tests/) — Manager tests: the session lifecycle, the bounded/unbound turn split, and the transcript the Sessions tab renders.
- [`completion_tests.rs`](./completion_tests.rs) — Tests for interactive turn-completion detection.
- [`input_tests.rs`](./input_tests.rs) — Driver-seam tests: task frames and session envelopes folded into one normalized turn, and the asymmetry between the two drivers.
- [`mod.rs`](./mod.rs) — Unit tests for the session model, split by surface so no file exceeds the repo's 500-line ceiling: `routing_tests` covers class/transport routing and the provider capability matrix; `registry_tests` the binding registry and turn serialization; `input_tests` the task-frame/envelope driver seam; `completion_tests` interactive turn-completion detection; and `manager_tests` the session lifecycle and turn execution.
- [`ops_tests.rs`](./ops_tests.rs) — Operator-op and data-model tests: parsing an "open session" line into a `SessionOp`, applying each op through the manager for its status line, and the trivial `impl`s on the session model types (class/policy/driver/phase, keys, records, and turn origins).
- [`registry_tests.rs`](./registry_tests.rs) — Binding-registry tests: the plan/record/reset lifecycle, per-provider isolation, LRU eviction, and which turns take the conversation chain.
- [`routing_tests.rs`](./routing_tests.rs) — Routing tests: which lifetime class a stimulus gets, and which transport follows from that class and the provider's capabilities.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
