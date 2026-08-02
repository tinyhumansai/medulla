# Harness

Read-only view-model helpers for the agent-harness contract: a compact task board rendering for a `HarnessStatus` payload, and a one-line budget note for an agent's `AgentBudgetMetadata` seat stamp. Pure formatting only — the `medulla-tui` crate turns the returned `Line`s / strings into ratatui spans.

## Contents

- [`mod.rs`](./mod.rs) — Read-only view-model helpers for the agent-harness contract: a compact task board rendering for a `HarnessStatus` payload, and a one-line budget note for an agent's `AgentBudgetMetadata` seat stamp. Pure formatting only — the `medulla-tui` crate turns the returned `Line`s / strings into ratatui spans.
- [`tests.rs`](./tests.rs) — Unit tests for the harness view-model helpers: board summary/lines and the read-only budget note.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
