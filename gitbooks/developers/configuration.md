# Configuration

Medulla reads a layered configuration, persists everything under a single home
directory, and selects one of three runtimes at startup. This page covers all
three.

## Medulla home

Everything Medulla persists lives under one home directory:

* Default: `~/.medulla`.
* Local dev: set `MEDULLA_DEV=1` (truthy is `1`/`true`, case-insensitive) and the
  home becomes `./.medulla` (relative to the cwd; gitignored).
* Explicit: `MEDULLA_HOME=<path>` overrides both.

Under the home:

* `credentials.json` — saved by [`medulla login`](authentication.md), mode `0600`.
* `config.toml` — the user-global config file.
* `state/` — the default `stateDir`, holding chat history under `chats/` and the
  resolved `core.sock`.
* `tinyplace/` — the default [tiny.place](https://tiny.place) identity directory.
* `worker.json` — the [worker profile](cli-reference.md#first-run-worker-registration).

A `.env` file in the current directory is loaded at startup, before anything
reads the environment: `KEY=VALUE` lines, `#` comments, an optional `export`
prefix, and single/double quotes are stripped. It never overrides variables
already set in the process environment — this is the usual way to opt into
`MEDULLA_DEV=1` for local dev.

## Layered config

Config is merged from lowest to highest precedence (highest wins):

1. Built-in defaults (production endpoints; `MEDULLA_STAGING` flips the default
   URLs).
2. User-global `<home>/config.toml`.
3. Project-local `./.medulla/config.toml` (else `./medulla.toml`).
4. Environment variables (`MEDULLA_API_URL`, `MEDULLA_TOKEN` via `tokenEnv`,
   `MEDULLA_STAGING`, `MEDULLA_STATE_DIR`, `TINYPLACE_*`).
5. CLI flags.

Files are merged field-by-field (a recursive table merge), so a project-local
file can override just `backend.baseUrl` without discarding the rest of a global
file. [TOML](https://toml.io/) is the primary format; `--config <path>` still
accepts either `.toml` or `.json` (parser chosen by extension) and bypasses file
discovery, but env vars and CLI flags still override it. The Config tab shows the
merged effective config and lists the source files that contributed.

Every section is optional; with no file anywhere, all defaults apply. Sections:
`backend`, `core`, `tinyplace` (identity/presence + peer roster for the daemon
and Overview panel), `stateDir` (default `<home>/state`; `MEDULLA_STATE_DIR`
overrides), `opencode` (worker display), `update` (`check = true`/`false` for the
background release check; `MEDULLA_NO_UPDATE_CHECK` env kill-switch), `theme` (TUI
colors — `primary`/`accent`/`selectionFg`/`dimBorder` as
[ratatui](https://ratatui.rs/) color names or `#rrggbb`; the Settings ›
Appearance subpage edits and persists these), and `medulla.contextWindowTokens`
(Context tab usage hint). Inference and tracing are server-side concerns — the
TUI has no config for them; unknown sections are ignored.

See
[`config.example.toml`](https://github.com/tinyhumansai/medulla/blob/main/config.example.toml)
for a commented reference and
[`src/sdk/src/config.rs`](https://github.com/tinyhumansai/medulla/tree/main/src/sdk/src/config)
for the full schema — fields are camelCase.

## Endpoints

The backend base URL defaults to production, `https://api.tinyhumans.ai`, and the
tiny.place endpoint to `https://api.tiny.place`. Set `MEDULLA_STAGING=1` (or
`true`, case-insensitive) to switch both defaults to their staging hosts
(`https://staging-api.tinyhumans.ai` and `https://staging-api.tiny.place`).

Base-URL precedence, highest first:

* **Backend:** `MEDULLA_API_URL` env var > config-file `backend.baseUrl` >
  staging/prod default.
* **tiny.place:** the `TINYPLACE_ENDPOINT` / `TINYPLACE_API_URL` /
  `NEXT_PUBLIC_API_URL` env chain > tinyplace config-file `endpoint` > config-file
  `tinyplace.baseUrl` > staging/prod default.

Override the base URL (and the token env var name) in the config file — e.g. to
point at a local backend:

```json
{
  "backend": {
    "baseUrl": "http://localhost:5000",
    "tokenEnv": "MEDULLA_TOKEN"
  }
}
```

An inline `"token"` field is also accepted, but keep secrets out of committed
files — prefer the env var.

## Runtimes

On startup the TUI picks one of three runtimes, in this order. If a preferred
runtime fails, it falls back down this chain and shows why in the status line.

1. **Core socket** — if `--core` is passed or the config has a `core` section,
   and the socket is reachable.
2. **Backend HTTP/SSE** — if a backend token is available (an inline
   `backend.token`, the `backend.tokenEnv` variable, or credentials saved by
   [`medulla login`](authentication.md)).
3. **Mock** — otherwise.

In the default (non-`--core`) path, when no token resolves the TUI does not drop
straight to the mock: it first opens the [login screen](authentication.md#logging-in-from-the-tui),
and the mock runtime is entered only if you press `m` to continue offline. An
explicit `--core` run keeps the plain backend→mock fallback and is never
redirected to the login screen.

### Mock (zero setup)

```sh
cargo run
```

With no token and no core socket, `medulla` opens a login screen; press `m` to
continue offline against the mock runtime — a scripted demo, no credentials, no
network, and the fastest way to explore the interface.

### Backend HTTP/SSE

Point the TUI at a running Medulla backend and give it a JWT:

```sh
MEDULLA_TOKEN=<jwt> medulla
```

Or log in through the browser — see [Authentication](authentication.md).

### Core socket

For driving a locally running core orchestration server over its
[NDJSON](https://ndjson.org/) Unix-socket protocol:

```sh
medulla --core
```

The socket path resolves as: `core.socketPath` from the config if set, else
`$XDG_RUNTIME_DIR/medulla/core.sock`, else `<stateDir>/core.sock` (the resolved
`stateDir`, which defaults to `<home>/state` and honors `MEDULLA_STATE_DIR`).
Config form:

```json
{
  "core": { "socketPath": "/tmp/medulla-core.sock" }
}
```

The core runtime unlocks the Workers tab (fleet peer management) and task
steering (`X` cancel task, `A` answer a pending question). It is **unix-only** (it
rides a Unix domain socket). On Windows a `--core` flag or `[core]` config section
resolves to a startup note ("core runtime requires unix sockets — unavailable on
Windows") and falls through to the normal backend→mock chain.
