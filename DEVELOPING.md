# Developing

Developer documentation for Medulla lives in the Developers section of the
GitBook docs — authored in the
[tinyhumansai/medulla](https://github.com/tinyhumansai/medulla) documentation
repository, so there is a single source of truth for build, run, and
architecture detail:

- [Developers overview](https://tinyhumans.gitbook.io/medulla/developers)
- [Getting Started](https://tinyhumans.gitbook.io/medulla/developers/getting-started) covers install, building from source, and the first run.
- [CLI Reference](https://tinyhumans.gitbook.io/medulla/developers/cli-reference) covers the TUI, `medulla daemon`, the `claude`/`codex`/`opencode` wrappers, and `medulla update`.
- [Configuration](https://tinyhumans.gitbook.io/medulla/developers/configuration) covers the Medulla home directory, layered config, and the three runtimes (core socket / backend / mock).
- [Authentication](https://tinyhumans.gitbook.io/medulla/developers/authentication) covers `medulla login`, tokens, and the loopback security model.
- [Architecture](https://tinyhumans.gitbook.io/medulla/developers/architecture) covers the SDK/TUI crate split, the `Runtime` trait, RLM, and the tiny.place bridge.
- [Contributing](https://tinyhumans.gitbook.io/medulla/developers/contributing) covers build, test, lint, coverage, and releasing.

The engineering specs this code refers to — `docs/host-link-protocol.md`,
`docs/workflows.md`, `docs/vendoring.md` and the rest — live in that same
repository under
[`docs/`](https://github.com/tinyhumansai/medulla/tree/main/docs). A bare
`docs/…` path in a source comment means a file there, not one here.

## Quick start

```sh
make init                       # submodules, rustfmt/clippy, locked deps, pre-push hook
cargo run                       # debug build, starts the TUI (login screen if signed out)
cargo test                      # unit + feature + e2e suites (all mocked, no network)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Run tests, Clippy, and formatting before pushing. See
[Contributing](https://tinyhumans.gitbook.io/medulla/developers/contributing)
for the full loop and the release process.

## Releasing

`Release` (`.github/workflows/release.yml`, dispatched manually) bumps the
version here, tags it here, builds the binary for every supported target, and
publishes the packaged artifacts — binaries, checksums, and the `latest.json`
update manifest `medulla update` reads — as a GitHub Release on the public
[tinyhumansai/medulla](https://github.com/tinyhumansai/medulla) repository. It
authenticates there with a GitHub App installation token scoped to that one
repository. Only build output crosses the boundary; no source is pushed there.
