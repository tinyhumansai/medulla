# Sessions

Interactive coding-agent session management: the two lifetime classes, the two turn-source drivers, and the machinery that runs them.

## Contents

- [`completion/`](./completion/) — Turn-completion detection for a harness driven through its **interactive** interface.
- [`input/`](./input/) — **The driver seam.** Sessions are driven from exactly two sources — a `medulla-task/1` task frame, or a `tinyplace.harness.session.v*` envelope — and this module is the one place that knows the difference.
- [`interactive/`](./interactive/) — The interactive transport: one long-lived harness process fed newline- delimited JSON turns over stdin.
- [`manager/`](./manager/) — `SessionManager` — the operator-facing surface the Sessions tab drives, and the daemon-facing entry that runs a folded `TurnRequest`.
- [`ops/`](./ops/) — `SessionOp` — the operator actions the Sessions screen dispatches, and the manager entry that applies one.
- [`registry/`](./registry/) — The session-binding registry: which harness session id a conversation is bound to, and the per-key serialization that keeps two turns from interleaving onto one transcript.
- [`routing/`](./routing/) — Session-class and transport routing: which lifetime a stimulus gets, and whether a provider can serve it interactively at all.
- [`tests/`](./tests/) — Unit tests for the session model, split by surface so no file exceeds the repo's 500-line ceiling: `routing_tests` covers class/transport routing and the provider capability matrix; `registry_tests` the binding registry and turn serialization; `input_tests` the task-frame/envelope driver seam; `completion_tests` interactive turn-completion detection; and `manager_tests` the session lifecycle and turn execution.
- [`turn_stream/`](./turn_stream/) — `TurnStream` — the mode-independent half of running a turn.
- [`mod.rs`](./mod.rs) — Interactive coding-agent session management: the two lifetime classes, the two turn-source drivers, and the machinery that runs them.
- [`turn_stream_tests.rs`](./turn_stream_tests.rs) — Unit tests for `TurnStream`, the mode-independent fold.
- [`types.rs`](./types.rs) — The session data model: lifetime class, turn-source driver, routing policy, the per-session record the UI renders, and the turn request/outcome pair.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
