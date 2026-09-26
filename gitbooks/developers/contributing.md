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
cargo run                       # debug build, starts the TUI
cargo run -- --mock             # debug build, offline demo runtime (no backend, no login)
cargo run --release             # optimized build
cargo install --path src/tui    # installs the `medulla` binary onto your PATH
```

`cargo run` with no flags uses the configured backend when a session is
available. Without usable credentials, it opens the TUI login screen; it does
not select mock mode. Use `cargo run -- --mock` explicitly for the offline mock
runtime.

## Validate

Run all three before pushing:

```sh
cargo test                              # unit + feature + e2e suites (all mocked, no network)
cargo clippy --all-targets -- -D warnings
cargo fmt --check                       # run `cargo fmt` to apply
```

The e2e suites spin up in-process stand-ins so they are safe anywhere. They all
live in `src/sdk/tests/support/`:

| File | What it stands in for |
| --- | --- |
| `mock_backend.rs` | The backend's HTTP and SSE surfaces. |
| `fake_app_server.rs` | The app-facing server endpoints. |
| `fake_provider.rs` | A coding-agent provider. |
| `mock_harness.rs`, `mock_harness_helpers.rs`, `mock_harness_script.rs`, `mock_harness_types.rs` | Mock `claude`/`codex`/`opencode` CLIs that emit realistic provider stream-JSONL, selected via the `MEDULLA_*_BIN` overrides. |
| `mock_openrouter.rs` | The OpenRouter inference endpoint. |

`mod.rs` wires them together and `README.md` documents them. See
[Architecture › Testing philosophy](architecture.md#testing-philosophy).

## Coverage

Coverage uses [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(requires `cargo install cargo-llvm-cov` + `rustup component add
llvm-tools-preview`):

```sh
cargo llvm-cov                    # run suite with coverage, print summary
cargo llvm-cov report --show-missing-lines
```

CI gates line coverage at 80% (`cargo llvm-cov --fail-under-lines 80`); keep new
code covered. `src/tui/src/main.rs` (the terminal event loop, which needs a real
TTY) and the daemon's live-network entry points are the known uncovered
remainder.

## Code style and file organization

* Standard [`rustfmt`](https://github.com/rust-lang/rustfmt) output (four-space
  indentation). `snake_case` modules/functions/files, `PascalCase` types/traits,
  `SCREAMING_SNAKE_CASE` constants.
* Prefer explicit error types at library boundaries and
  [`anyhow`](https://docs.rs/anyhow/) for binary orchestration.
* A 500-line ceiling per `.rs` file. When a file approaches the limit, split it
  into a directory module (`foo.rs` becomes `foo/mod.rs` plus focused
  submodules), with `mod.rs` kept thin. Data types live in a `types.rs`
  submodule; unit tests live in a sibling `tests.rs`.
* Document generously: a `//!` module doc on every module, a `///` doc on every
  public item, comments on non-trivial private functions, explaining the *why*
  and not the mechanically obvious.

The authoritative rules live in the repository's
[`AGENTS.md`](https://github.com/tinyhumansai/medulla/blob/main/AGENTS.md).

## Commits and pull requests

History uses concise [conventional](https://www.conventionalcommits.org/)
subjects: `test(ui): ...`, `refactor: ...`, `docs: ...`. Keep commits narrow and
imperative. PRs should summarize behavior, identify configuration or public API
changes, link relevant issues, and list the validation commands they cover.
Include screenshots only for visible TUI changes.

## Releasing

The **Release** workflow is dispatched from `main`. It reads the workspace
version, creates the matching `vX.Y.Z` tag, builds the supported platform
binaries, and publishes the packages and `latest.json` as a GitHub Release in
this repository. It uses the repository's standard `GITHUB_TOKEN` and requires
no organization-only app secrets. Bump the workspace version and lockfile in a
reviewed pull request before dispatching a release.

See [DEVELOPING.md](../../DEVELOPING.md) for the repository development overview.
