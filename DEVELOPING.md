# Developing

How to build and run the `medulla` TUI locally from this repo.

## Initialize the repository

Run the initialization target once after cloning:

```sh
make init
```

This initializes vendored submodules, installs the Rustfmt and Clippy components,
fetches locked dependencies, and enables the repository's pre-push hook. The hook
checks Rust formatting and runs Clippy with warnings denied.

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

Other subcommands: `medulla daemon` (headless coding-agent daemon over tiny.place), `medulla codex` / `medulla claude` / `medulla opencode` (run the coding-agent CLI in your terminal, bridged to tiny.place — see below), `medulla sessions` (recent claude/codex sessions as JSON), `medulla version`, `medulla help`.

## Wrapper commands (`medulla codex` / `claude` / `opencode`)

These launch the real coding-agent CLI in your terminal exactly as if you had run
it directly — inherited stdio, unrecognized flags passed through verbatim — while
bridging the session to tiny.place underneath. The wrapper tails the harness's own
JSONL transcript, normalizes each record into a typed `SessionEnvelopeV2` event,
and forwards the stream as encrypted Signal DMs to the configured owner; with
inbound input enabled it also polls for owner→session control frames and types
their text into the child.

```sh
medulla codex resume            # any args after the provider go to the CLI verbatim
medulla claude --model opus-4   # unrecognized flags pass straight through
medulla codex --no-bridge       # pure passthrough: run the CLI with no tiny.place bridge
medulla codex -- --no-bridge    # `--` forces everything after it to the child
```

Configuration is by environment variable (mirrors the tinyplace CLI):

| Variable | Effect |
| --- | --- |
| `TINYPLACE_HARNESS_DM_TO` / `TINYPLACE_<P>_DM_TO` / `TINYPLACE_OPENHUMAN_OWNER` | tiny.place owner to forward the session envelopes to |
| `TINYPLACE_HARNESS_RECEIVE_FROM` / `TINYPLACE_<P>_RECEIVE_FROM` | peer whose input control frames / plain DMs are injected (defaults to the owner) |
| `TINYPLACE_HARNESS_RECEIVE=0` / `TINYPLACE_<P>_RECEIVE=0` | disable inbound input injection |
| `TINYPLACE_<P>_BIN` (`TINYPLACE_CODEX_BIN`, `TINYVERSE_CLAUDE_BIN`/`TINYPLACE_CLAUDE_BIN`, `TINYPLACE_OPENCODE_BIN`) | override the provider binary |
| `TINYPLACE_<P>_SESSIONS_DIR` | override the transcript directory the tailer watches |

If no owner is configured (and `--no-bridge` was not passed), the wrapper prints a
single warning and runs as a plain passthrough.

Scope notes: this is the single-terminal `--raw` wrapper. It does not build the
tinyplace TUI chrome, the `--agent` plugin mode, the machine-bus multi-terminal
coordination, the opencode SSE server, or the terminal-envelope writer. stdio is
inherited (no PTY): for a pristine full-screen TUI, run without inbound input (or
`--no-bridge`) so stdin stays attached to the terminal — enabling input injection
pipes stdin as a best-effort byte pump. `medulla opencode` runs as a passthrough
with input injection but no transcript tailing (its session log is not a flat
JSONL the mappers read).

TUI flags:

| Flag | Effect |
| --- | --- |
| `--config <path>` | Path to the config file (default: `medulla.tui.json` in the cwd) |
| `--core` | Drive the core orchestration server over its Unix socket |
| `--no-alt-screen` | Stay on the main screen buffer (useful for scrollback while debugging) |

## Runtimes

On startup the TUI picks one of three runtimes, in this order:

1. **Core socket** — if `--core` is passed or the config has a `core` section, and the socket is reachable.
2. **Backend HTTP/SSE** — if a backend token is available (an inline `backend.token`, the `backend.tokenEnv` variable, or credentials saved by `medulla login`).
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

The backend base URL defaults to the production API, `https://api.tinyhumans.ai`, and the tiny.place endpoint to `https://api.tiny.place`. Set `MEDULLA_STAGING=1` (or `true`, case-insensitive) to switch both defaults to their staging hosts (`https://staging-api.tinyhumans.ai` and `https://staging-api.tiny.place`).

Base-URL precedence, highest first:

- **Backend:** `MEDULLA_API_URL` env var > config-file `backend.baseUrl` > staging/prod default.
- **tiny.place:** the existing `TINYPLACE_ENDPOINT` / `TINYPLACE_API_URL` / `NEXT_PUBLIC_API_URL` env chain > tinyplace config-file `endpoint` > config-file `tinyplace.baseUrl` > staging/prod default.

Override the base URL (and the token env var name) in the config file — e.g. to point at a local backend:

```json
{
  "backend": {
    "baseUrl": "http://localhost:5000",
    "tokenEnv": "MEDULLA_TOKEN"
  }
}
```

An inline `"token"` field is also accepted but keep secrets out of committed files — prefer the env var.

### Logging in (`medulla login`)

Instead of managing a JWT by hand, log in through the browser:

```sh
medulla login                       # google by default; opens the browser
medulla login --provider github     # google | github | twitter | discord
medulla login --no-browser          # just print the URL to open yourself
medulla login --token <64-hex>      # headless: redeem a one-time login token
```

`login` runs an RFC 8252 loopback flow: it binds a local `127.0.0.1:<port>` listener, sends you to the backend's OAuth page, and captures the JWT the backend redirects back with. It then verifies the token via `/auth/me`, prints who you are, and saves credentials to `<config-dir>/medulla/credentials.json` (mode `0600` on unix) — for example `~/Library/Application Support/medulla/credentials.json` on macOS or `~/.config/medulla/credentials.json` on Linux. The base URL comes from `backend.baseUrl` in the config (`--config <path>` to point at a different config).

On the next `medulla` run the TUI uses those stored credentials automatically, provided their `baseUrl` matches the configured backend. `medulla logout` clears the file. Precedence for the backend token stays: inline `backend.token` > `backend.tokenEnv` > stored credentials.

The loopback listener hardens the callback against a hostile page sharing the same `127.0.0.1` origin: a random 32-hex state nonce is appended to the `redirectUri` before it reaches the backend, and the listener rejects any `/auth` callback whose `state` is missing or mismatched (HTTP 400) while continuing to wait. It also drops non-loopback peers, replies 405 to non-GET and 404 to non-`/auth` requests, and bounds each connection with a 5s read timeout and an 8 KiB buffer.

### Logging in from the TUI

When you start `medulla` without `--core` and no token resolves — or the stored/env token is expired or rejected (`me()` preflight fails with an auth error) — the TUI opens a login screen before the main app instead of silently dropping to the mock:

- **Enter / `o`** — start the browser loopback flow. The screen shows the login URL and waits for the callback on `127.0.0.1:<port>`; **Esc** cancels.
- **←/→** or **`p`** — cycle the provider (google / github / twitter / discord).
- **`t`** — paste a JWT or a 64-hex one-time login token (64 lowercase hex is redeemed via `/auth/login-token/consume`, anything else is treated as a JWT). **Enter** submits, **Esc** cancels.
- **`m`** — continue offline with the mock runtime. **`q`** / **Ctrl-C** — quit.

On a token from either path the TUI verifies it via `/auth/me`, flashes who you are, saves the credentials (a save failure is a non-fatal notice), and proceeds into the app with a backend runtime. Explicit `--core` runs are never redirected to this screen.

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
