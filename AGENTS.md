# Repository Guidelines

## Project Structure & Module Organization

This single Rust crate provides the `medulla` library and CLI/TUI. Keep reusable APIs in `src/lib.rs` modules and process wiring in `src/main.rs`.

- `src/client/` implements the backend HTTP/SSE client and protocol types.
- `src/runtime/` contains backend, core-socket, and scripted mock runtime adapters.
- `src/ui/` owns ratatui state, rendering, input, and chat persistence.
- `src/daemon/` and `src/tinyplace_support/` implement provider and tiny.place integration.
- `tests/` contains feature and mocked end-to-end suites; reusable stand-ins live in `tests/support/`.
- `vendor/tinyplace/` is vendored upstream code. Avoid unrelated edits there.
- `target/` and `.medulla-state/` are generated or local runtime data; never commit them.

## Build, Test, and Development Commands

- `cargo run` starts the TUI with the mock runtime when no credentials are set.
- `cargo run --release` runs an optimized build.
- `cargo install --path .` installs the `medulla` binary.
- `cargo test` runs unit, feature, and mocked end-to-end tests without live network access.
- `cargo clippy --all-targets -- -D warnings` treats all lint warnings as failures.
- `cargo fmt --check` verifies formatting; run `cargo fmt` to apply it.
- `cargo llvm-cov` reports coverage when `cargo-llvm-cov` and `llvm-tools-preview` are installed.

Run tests, Clippy, and formatting before handoff.

## Coding Style & Naming Conventions

Use standard `rustfmt` output (four-space indentation). Name modules, functions, and files in `snake_case`; types and traits in `PascalCase`; constants in `SCREAMING_SNAKE_CASE`. Prefer explicit error types at library boundaries and `anyhow` for binary orchestration. Keep imports grouped at the top and comments sparse, direct, and focused on non-obvious behavior.

## Testing Guidelines

Place focused unit tests beside their module and cross-module behavior in `tests/`. Name integration files by behavior, such as `e2e_core.rs` or `feature_workers.rs`. Use the mock backend, core socket, tiny.place server, and harness CLIs in `tests/support/`; tests must remain deterministic and offline. Maintain coverage near the documented 92% line baseline and cover new branches.

## Commit & Pull Request Guidelines

History uses concise conventional subjects, for example `test(ui): ...`, `refactor: ...`, and `docs: ...`. Keep commits narrow and imperative. PRs should summarize behavior, identify configuration or public API changes, link relevant issues, and list validation commands. Include screenshots only for visible TUI changes.

## Security & Configuration

Never commit tokens, `.env`, `medulla.tui.json` secrets, or runtime state. Prefer `MEDULLA_TOKEN` and documented environment variables over inline credentials. Treat Unix-socket paths and provider binary overrides as untrusted configuration and validate them at boundaries.
