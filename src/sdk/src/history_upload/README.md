# History Upload

Sharing local coding-agent history to earn onboarding credit.

## Contents

- [`share/`](./share/) — Uploading a scanned history and claiming the reward.
- [`mod.rs`](./mod.rs) — Sharing local coding-agent history to earn onboarding credit.
- [`redact.rs`](./redact.rs) — Client-side secret scrubbing, applied before any transcript leaves the machine.
- [`scan.rs`](./scan.rs) — Locating the transcripts that will be shared, and reading them off disk.
- [`tests.rs`](./tests.rs) — Unit tests for history sharing.
- [`types.rs`](./types.rs) — Data model for the history-sharing reward flow: what scanning found on disk, the per-agent tallies the consent screen shows, and a transcript that has been read and scrubbed ready for upload.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
