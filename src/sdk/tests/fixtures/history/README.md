# History

Captured history samples used to verify session discovery, parsing, redaction, and upload behavior.

## Contents

- [`claude-session.jsonl`](./claude-session.jsonl) — Provides the Claude Session captured-history fixture for deterministic tests.
- [`codex-rollout.jsonl`](./codex-rollout.jsonl) — Provides the Codex Rollout captured-history fixture for deterministic tests.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
