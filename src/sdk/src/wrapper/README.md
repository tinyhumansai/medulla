# Wrapper

The transparent harness wrapper behind `medulla codex` / `medulla claude` / `medulla opencode`.

## Contents

- [`bridge/`](./bridge/) — The tiny.place bridge for one wrapped session and its I/O helpers.
- [`envelope/`](./envelope/) — v2 session-envelope construction for the harness wrapper.
- [`run/`](./run/) — Process orchestration for a wrapped session: the `run_wrapper` entry point, the `run_wrapper_with` core loop that drives the child CLI and the tiny.place `Bridge`, and the exit-code / signal plumbing around it.
- [`tail/`](./tail/) — Session-log discovery and tailing for the wrapper.
- [`args.rs`](./args.rs) — Command-line parsing for the wrapper entry point: split the wrapper's own flags from the arguments passed through to the child CLI.
- [`control_tests.rs`](./control_tests.rs) — Tests for the control module.
- [`control.rs`](./control.rs) — Owner→wrapper control-frame targeting.
- [`mod.rs`](./mod.rs) — The transparent harness wrapper behind `medulla codex` / `medulla claude` / `medulla opencode`.
- [`tests.rs`](./tests.rs) — Unit tests for the wrapper root: argument parsing, provider→agent-kind mapping, recipient resolution precedence, session-id minting, and the missing-binary error path.
- [`types.rs`](./types.rs) — Data model for a wrapped session: the caller-facing `WrapperConfig` and the internal `WrapperTimings` resolved from the environment. These are pure data plus their trivial constructors; the behaviour that consumes them lives in `run` and `bridge`.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
