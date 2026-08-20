# Developing

Developer documentation for Medulla lives in the Developers section of the
GitBook docs, so there is a single source of truth for build, run, and
architecture detail:

- [Developers overview](gitbooks/developers/README.md)
- [Getting Started](gitbooks/developers/getting-started.md) covers install, building from source, and the first run.
- [CLI Reference](gitbooks/developers/cli-reference.md) covers the TUI, `medulla daemon`, the `claude`/`codex`/`opencode` wrappers, and `medulla update`.
- [Configuration](gitbooks/developers/configuration.md) covers the Medulla home directory, layered config, and the three runtimes (core socket / backend / mock).
- [Authentication](gitbooks/developers/authentication.md) covers `medulla login`, tokens, and the loopback security model.
- [Architecture](gitbooks/developers/architecture.md) covers the SDK/TUI crate split, the `Runtime` trait, RLM, and the tiny.place bridge.
- [Contributing](gitbooks/developers/contributing.md) covers build, test, lint, coverage, and releasing.

## Quick start

```sh
make init                       # submodules, rustfmt/clippy, locked deps, pre-push hook
cargo run                       # debug build, starts the TUI (login screen if signed out)
cargo test                      # unit + feature + e2e suites (all mocked, no network)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Run tests, Clippy, and formatting before pushing. See
[Contributing](gitbooks/developers/contributing.md) for the full loop and the
release process.
