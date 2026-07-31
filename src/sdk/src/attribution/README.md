# Attribution

Git commit attribution for Medulla-launched harnesses.

## Configuration

Config-driven, on by default. Turn it off in `medulla.tui.json`:

```json
{ "attribution": { "commit": false } }
```

Callers resolve the value from the loaded config and pass it to
`attribution_args` / `attribution_env`; this module never reads the environment
or the filesystem to decide.

## Contents

- [`mod.rs`](./mod.rs) — Git commit attribution for Medulla-launched harnesses.
- [`prepare_commit_msg.rs`](./prepare_commit_msg.rs) — Generate the `prepare-commit-msg` git hook that adds the `Co-authored-by` trailer from the `MEDULLA_ATTRIBUTION` environment variable. This is the mechanism of record for *every* provider: a harness CLI's own attribution setting (where one exists) only asks the model to write the trailer, so it drops out whenever the task brief dictates a commit message.
- [`tests.rs`](./tests.rs) — Unit tests for `super::attribution`: trailer shape, the config-driven on/off switch, per-provider coverage, and end-to-end hook behaviour driven through a real `git commit` (explicit messages, amend idempotency, trailer-block placement, chaining to the repo's own hook, merge exclusion).

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
