# Contributing

How to build, validate, and release the Medulla workspace. The
[getting-started](getting-started.md) page covers a first build; this page is the
day-to-day development loop.

## Initialize

Run the initialization target once after cloning:

```sh
make init
```

This initializes vendored submodules, installs the [Rustfmt](https://github.com/rust-lang/rustfmt)
and [Clippy](https://doc.rust-lang.org/clippy/) components, fetches locked
dependencies, and enables the repository's pre-push hook. The hook checks Rust
formatting and runs Clippy with warnings denied.

## Build and run

```sh
cargo run                       # debug build, starts the TUI (mock runtime)
cargo run --release             # optimized build
cargo install --path src/tui    # installs the `medulla` binary onto your PATH
```

## Validate

Run all three before pushing:

```sh
cargo test                              # unit + feature + e2e suites (all mocked, no network)
cargo clippy --all-targets -- -D warnings
cargo fmt --check                       # run `cargo fmt` to apply
```

The e2e suites spin up in-process stand-ins so they are safe anywhere: a mock
HTTP/SSE backend (`src/sdk/tests/support/mock_backend.rs`), a mock core
Unix-socket server (`mock_core.rs`), a mock tiny.place API server
(`mock_tinyplace.rs`), and mock `claude`/`codex`/`opencode` CLIs that emit
realistic provider stream-JSONL (`mock_harness.rs`, selected via the
`TINYPLACE_*_BIN` overrides). See [Architecture › Testing philosophy](architecture.md#testing-philosophy).

## Coverage

Coverage uses [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(requires `cargo install cargo-llvm-cov` + `rustup component add
llvm-tools-preview`):

```sh
cargo llvm-cov                    # run suite with coverage, print summary
cargo llvm-cov report --show-missing-lines
```

CI gates line coverage at 90% (`cargo llvm-cov --fail-under-lines 90`); keep new
code covered. `src/tui/src/main.rs` (the terminal event loop, which needs a real
TTY) and the daemon's live-network entry points are the known uncovered
remainder.

## Code style and file organization

* Standard [`rustfmt`](https://github.com/rust-lang/rustfmt) output (four-space
  indentation). `snake_case` modules/functions/files, `PascalCase` types/traits,
  `SCREAMING_SNAKE_CASE` constants.
* Prefer explicit error types at library boundaries and
  [`anyhow`](https://docs.rs/anyhow/) for binary orchestration.
* **500-line ceiling** per `.rs` file. When a file approaches the limit, split it
  into a directory module (`foo.rs` → `foo/mod.rs` plus focused submodules), with
  `mod.rs` kept thin. Data types live in a `types.rs` submodule; unit tests live
  in a sibling `tests.rs`.
* Document generously: a `//!` module doc on every module, a `///` doc on every
  public item, comments on non-trivial private functions — explaining the *why*,
  not the mechanically obvious.

The authoritative rules live in the repository's
[`AGENTS.md`](https://github.com/tinyhumansai/medulla/blob/main/AGENTS.md).

## Commits and pull requests

History uses concise [conventional](https://www.conventionalcommits.org/) subjects
— `test(ui): ...`, `refactor: ...`, `docs: ...`. Keep commits narrow and
imperative. PRs should summarize behavior, identify configuration or public API
changes, link relevant issues, and list the validation commands they cover.
Include screenshots only for visible TUI changes.

## Releasing

Releases are one-click: run the **Release** workflow (Actions → Release → Run
workflow) and pick a [semver](https://semver.org/) bump. It bumps
`Cargo.toml`/`Cargo.lock`, commits and tags on `main`, builds all targets, and
publishes the GitHub Release with the `latest.json` update manifest that
[`medulla update`](cli-reference.md#medulla-update) reads.

`main` is branch-protected (the four CI checks are required), and the workflow's
default `GITHUB_TOKEN` cannot bypass that — the version-bump push would be
rejected. The `tag` job therefore runs against the **`Production`** environment
and authenticates as the org's GitHub App: it mints a short-lived installation
token from the `XGITHUB_APP_ID` / `XGITHUB_APP_PRIVATE_KEY` environment secrets
and pushes the bump commit + tag with it. The app needs **Contents: read and
write** and must be able to bypass the protection rule; every other path to `main`
still goes through a PR with green checks.

## Security and configuration

Never commit tokens, `.env`, `medulla.tui.json` secrets, or runtime state. Prefer
`MEDULLA_TOKEN` and documented environment variables over inline credentials.
Treat Unix-socket paths and provider binary overrides as untrusted configuration
and validate them at boundaries.
