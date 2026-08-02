# Session History

Recent-session history for local harness sessions.

## Contents

- [`list.rs`](./list.rs) — The ranked recent-sessions read model — `list_recent_sessions`, the entry point behind `medulla sessions` and the resume pane.
- [`mod.rs`](./mod.rs) — Recent-session history for local harness sessions.
- [`scan.rs`](./scan.rs) — Filesystem scanning and discovery: locating each agent's session directory, enumerating the transcript files inside it, and finding the newest file that belongs to a just-launched session.
- [`summary.rs`](./summary.rs) — Reading a session file's head window and distilling it into a `SessionSummary`: the session id, recorded cwd, and a display label taken from the first human prompt.
- [`tests.rs`](./tests.rs) — Unit tests for recent-session scanning, summary parsing, label extraction, and current-folder-first ranking.
- [`types.rs`](./types.rs) — Data model for recent-session history: the agent kind, the public `RecentSession` row, and the internal staging types shared across the scanning and summary submodules.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
