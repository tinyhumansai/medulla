# Update

Release update checking and self-update.

## Contents

- [`check.rs`](./check.rs) — The pure core of update checking: manifest parsing, semver comparison, platform selection, and the thin network fetch that resolves them into an `UpdateInfo`.
- [`install_tests.rs`](./install_tests.rs) — Tests for the install module.
- [`install.rs`](./install.rs) — The thin IO half of self-update: downloading and verifying an asset, extracting the archive, atomically swapping the running binary, and the `run_update` entry point that drives `medulla update [--check]`.
- [`mod.rs`](./mod.rs) — Release update checking and self-update.
- [`tests.rs`](./tests.rs) — Unit tests for update version comparison, manifest parsing, platform selection, hashing, and the atomic binary install.
- [`types.rs`](./types.rs) — The update data model: the release `latest.json` manifest, its per-platform asset entries, and the resolved `UpdateInfo` a check produces.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
