# Developing

How to build and run the `medulla` TUI locally from this repo.

## Prerequisites

- Rust stable (edition 2021) via [rustup](https://rustup.rs/)
- A real terminal — the TUI refuses to start without a TTY. Kitty-protocol terminals (kitty, WezTerm, Ghostty, recent iTerm2) additionally get Shift-Enter for newlines in the composer.

## Build and run

```sh
cargo run                 # debug build, starts the TUI
cargo run --release       # optimized build
cargo install --path .    # installs the `medulla` binary onto your PATH
medulla                   # bare invocation starts the TUI
```

Other subcommands: `medulla daemon` (headless coding-agent daemon over tiny.place), `medulla sessions` (recent claude/codex sessions as JSON), `medulla version`, `medulla help`.

TUI flags:

| Flag | Effect |
| --- | --- |
| `--config <path>` | Path to the config file (default: `medulla.tui.json` in the cwd) |
| `--core` | Drive the core orchestration server over its Unix socket |
| `--no-alt-screen` | Stay on the main screen buffer (useful for scrollback while debugging) |

## Runtimes

On startup the TUI picks one of three runtimes, in this order:

1. **Core socket** — if `--core` is passed or the config has a `core` section, and the socket is reachable.
2. **Backend HTTP/SSE** — if a backend token is available.
3. **Mock** — otherwise. A scripted demo runtime: no credentials, no network. If a preferred runtime fails, the TUI falls back down this chain and shows why in the status line.

### Mock (zero setup)

```sh
cargo run
```

With no token and no core socket you land in the mock runtime — the fastest way to explore the interface. Press `?` or open the Help tab for keybindings.

### Backend HTTP/SSE

Point the TUI at a running Medulla backend and give it a JWT:

```sh
MEDULLA_TOKEN=<jwt> medulla
```

The base URL defaults to `http://localhost:5000`; override it (and the token env var name) in the config file:

```json
{
  "backend": {
    "baseUrl": "http://localhost:5000",
    "tokenEnv": "MEDULLA_TOKEN"
  }
}
```

An inline `"token"` field is also accepted but keep secrets out of committed files — prefer the env var.

### Core socket

For driving a locally running core orchestration server over its NDJSON Unix-socket protocol:

```sh
medulla --core
```

The socket path resolves as: `core.socketPath` from the config if set, else `$XDG_RUNTIME_DIR/medulla/core.sock`, else `$MEDULLA_STATE_DIR/core.sock`. Config form:

```json
{
  "core": { "socketPath": "/tmp/medulla-core.sock" }
}
```

The core runtime unlocks the Workers tab (fleet peer management) and task steering (`X` cancel task, `A` answer a pending question).

## Configuration

The TUI reads `medulla.tui.json` from the current directory (or `--config <path>`). Every section is optional; an absent file just means all defaults. Sections: `backend`, `core`, `tinyplace` (identity/presence + peer roster for the daemon and Overview panel), `stateDir` (default `.medulla-state/`, holds chat history under `chats/`), `opencode` (worker display), and `medulla.contextWindowTokens` (Context tab usage hint). Inference and tracing are server-side concerns — the TUI has no config for them; unknown sections in existing config files are ignored. See `src/config.rs` for the full schema — fields are camelCase.

## Validation

```sh
cargo test                              # unit + feature + e2e suites (all mocked, no network)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Run tests and clippy before pushing. The e2e suites spin up in-process stand-ins so they are safe anywhere: a mock HTTP/SSE backend (`tests/support/mock_backend.rs`), a mock core Unix-socket server (`mock_core.rs`), a mock tiny.place API server (`mock_tinyplace.rs`), and mock `claude`/`codex`/`opencode` CLIs that emit realistic provider stream-JSONL (`mock_harness.rs`, selected via the `TINYPLACE_*_BIN` overrides).

Coverage (requires `cargo install cargo-llvm-cov` + `rustup component add llvm-tools-preview`):

```sh
cargo llvm-cov                    # run suite with coverage, print summary
cargo llvm-cov report --show-missing-lines
```

The suite holds ~92% line coverage; keep new code covered. `src/main.rs` (the terminal event loop, needs a real TTY) and the daemon's live-network entry points are the known uncovered remainder.
