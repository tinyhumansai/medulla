# Attribution

Git commit attribution for Medulla-launched harnesses.

## Contents

- [`mod.rs`](./mod.rs) — Git commit attribution for Medulla-launched harnesses.
- [`prepare_commit_msg.rs`](./prepare_commit_msg.rs) — Generate a `prepare-commit-msg` git hook that appends a `Co-authored-by` trailer from the `MEDULLA_ATTRIBUTION` environment variable. Used for providers whose CLI has no built-in attribution knob (Codex, Opencode).
- [`tests.rs`](./tests.rs) — Unit tests for `super::attribution`: trailer shape, the kill-switch precedence matrix, and per-provider coverage.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
