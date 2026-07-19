# Repository Guidelines

## Project Structure & Module Organization

This repository is a two-crate Cargo workspace: the `medulla` SDK library at `src/sdk/` and the `medulla-tui` app crate at `src/tui/` (which ships the `medulla` binary). Keep reusable APIs in the SDK; keep rendering and process wiring in the app crate.

- `src/sdk/src/client/` implements the backend HTTP/SSE client and protocol types.
- `src/sdk/src/runtime/` contains backend, core-socket, and scripted mock runtime adapters.
- `src/sdk/src/daemon/` and `src/sdk/src/tinyplace_support/` implement provider and tiny.place integration.
- `src/sdk/src/ui/` holds the UI-facing data surface (events, agent lanes, chat store, onboarding screen, util); the app crate re-exports it under `crate::ui`.
- `src/tui/src/ui/` owns ratatui state, rendering, input, and theming; `src/tui/src/cli.rs` owns argument parsing; `src/tui/src/main.rs` owns process wiring.
- `src/sdk/tests/` and `src/tui/tests/` contain feature and mocked end-to-end suites; reusable stand-ins live in `src/sdk/tests/support/` (the app crate's tests reach them via `#[path]`).
- `vendor/tinyplace/` is vendored upstream code, excluded from the workspace. Avoid unrelated edits there.
- `target/` and `.medulla-state/` are generated or local runtime data; never commit them.

## Build, Test, and Development Commands

- `cargo run` starts the TUI with the mock runtime when no credentials are set.
- `cargo run --release` runs an optimized build.
- `cargo install --path src/tui` installs the `medulla` binary.
- `cargo test` runs unit, feature, and mocked end-to-end tests for both crates without live network access.
- `cargo clippy --all-targets -- -D warnings` treats all lint warnings as failures.
- `cargo fmt --check` verifies formatting; run `cargo fmt` to apply it.
- `cargo llvm-cov` reports coverage when `cargo-llvm-cov` and `llvm-tools-preview` are installed.

Run tests, Clippy, and formatting before handoff.

## Coding Style & Naming Conventions

Use standard `rustfmt` output (four-space indentation). Name modules, functions, and files in `snake_case`; types and traits in `PascalCase`; constants in `SCREAMING_SNAKE_CASE`. Prefer explicit error types at library boundaries and `anyhow` for binary orchestration. Keep imports grouped at the top and comments sparse, direct, and focused on non-obvious behavior.

## Testing Guidelines

Place focused unit tests beside their module and cross-module behavior in the owning crate's `tests/` directory (`src/sdk/tests/` or `src/tui/tests/`). Name integration files by behavior, such as `e2e_core.rs` or `feature_workers.rs`. Use the mock backend, core socket, tiny.place server, and harness CLIs in `src/sdk/tests/support/`; tests must remain deterministic and offline. Maintain coverage near the documented 92% line baseline and cover new branches.

## Commit & Pull Request Guidelines

History uses concise conventional subjects, for example `test(ui): ...`, `refactor: ...`, and `docs: ...`. Keep commits narrow and imperative. PRs should summarize behavior, identify configuration or public API changes, link relevant issues, and list validation commands. Include screenshots only for visible TUI changes.

## Security & Configuration

Never commit tokens, `.env`, `medulla.tui.json` secrets, or runtime state. Prefer `MEDULLA_TOKEN` and documented environment variables over inline credentials. Treat Unix-socket paths and provider binary overrides as untrusted configuration and validate them at boundaries.
