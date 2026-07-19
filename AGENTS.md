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

Use standard `rustfmt` output (four-space indentation). Name modules, functions, and files in `snake_case`; types and traits in `PascalCase`; constants in `SCREAMING_SNAKE_CASE`. Prefer explicit error types at library boundaries and `anyhow` for binary orchestration. Keep imports grouped at the top.

## File Organization & Size

These rules are mandatory for all new and edited Rust source. Split proactively rather than letting a file grow past the limit.

- **500-line ceiling.** No `.rs` file should exceed 500 lines. When a file approaches the limit, split it into a directory module (`foo.rs` → `foo/mod.rs` plus focused submodules). `mod.rs` stays thin: module docs, `mod`/`pub use` wiring, and only glue that fits no more specific submodule.
- **Types in `types.rs`.** A module's data types (structs, enums, type aliases) and their trivial `impl`s live in a `types.rs` submodule, re-exported from `mod.rs`. Behaviour-heavy `impl`s may live beside the logic that uses them when that reads more clearly.
- **Tests in `tests.rs`.** Unit tests for a module live in a sibling `tests.rs` (declared `#[cfg(test)] mod tests;`), not inline at the bottom of the logic file. Cross-module and end-to-end tests stay in the crate's `tests/` directory.
- **Split by responsibility.** Group submodules by cohesive purpose (parsing, resolution, rendering, persistence), not arbitrary line count. Each submodule states its single purpose in its module doc.

## Documentation

Document generously — explain intent and non-obvious behaviour rather than restating code.

- **Every module** gets a `//!` doc comment stating its responsibility and how it fits the crate.
- **Every public item** (functions, types, traits, fields, variants) gets a `///` doc comment. Public functions note important preconditions, side effects, and error conditions.
- **Non-trivial private functions** get a `///` or `//` comment describing what they do and why.
- Keep prose direct; document the *why*, not the mechanically obvious.

## Testing Guidelines

Place focused unit tests in a module's sibling `tests.rs` and cross-module behavior in the owning crate's `tests/` directory (`src/sdk/tests/` or `src/tui/tests/`). Name integration files by behavior, such as `e2e_core.rs` or `feature_workers.rs`. Use the mock backend, core socket, tiny.place server, and harness CLIs in `src/sdk/tests/support/`; tests must remain deterministic and offline. Maintain coverage near the documented 92% line baseline and cover new branches.

## Commit & Pull Request Guidelines

History uses concise conventional subjects, for example `test(ui): ...`, `refactor: ...`, and `docs: ...`. Keep commits narrow and imperative. PRs should summarize behavior, identify configuration or public API changes, link relevant issues, and list validation commands. Include screenshots only for visible TUI changes.

## Security & Configuration

Never commit tokens, `.env`, `medulla.tui.json` secrets, or runtime state. Prefer `MEDULLA_TOKEN` and documented environment variables over inline credentials. Treat Unix-socket paths and provider binary overrides as untrusted configuration and validate them at boundaries.
