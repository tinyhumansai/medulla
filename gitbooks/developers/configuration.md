# Configuration

Medulla reads a layered configuration, persists everything under a single home directory, and selects one of three runtimes at startup. This page covers all three.

## Medulla home

Everything Medulla persists lives under one home directory:

* Default: `~/.medulla`.
* Local dev: set `MEDULLA_DEV=1` (truthy is `1`/`true`, case-insensitive) and the home becomes `./.medulla` (relative to the cwd; gitignored).
* Explicit: `MEDULLA_HOME=<path>` overrides both.

Under the home:

* `credentials.json` — saved by [`medulla login`](authentication.md), mode `0600`.
* `config.toml` — the user-global config file.
* `state/` — the default `stateDir`, holding chat history under `chats/` and the resolved `core.sock`.
* `tinyplace/` — the default [tiny.place](https://tiny.place) identity directory.
* `worker.json` — the [worker profile](cli-reference.md#first-run-worker-registration).

A `.env` file in the current directory is loaded at startup, before anything reads the environment: `KEY=VALUE` lines, `#` comments, an optional `export` prefix, and single/double quotes are stripped. It never overrides variables already set in the process environment — this is the usual way to opt into `MEDULLA_DEV=1` for local dev.

## Layered config

Config is merged from lowest to highest precedence (highest wins):

1. Built-in defaults (production endpoints; `MEDULLA_STAGING` flips the default URLs).
2. User-global `<home>/config.toml`.
3. Project-local `./.medulla/config.toml` (else `./medulla.toml`).
4. Environment variables (`MEDULLA_API_URL`, `MEDULLA_TOKEN` via `tokenEnv`, `MEDULLA_STAGING`, `MEDULLA_STATE_DIR`, `TINYPLACE_*`).
5. CLI flags.

Files are merged field-by-field (a recursive table merge), so a project-local file can override just `backend.baseUrl` without discarding the rest of a global file. [TOML](https://toml.io/) is the primary format; `--config <path>` still accepts either `.toml` or `.json` (parser chosen by extension) and bypasses file discovery, but env vars and CLI flags still override it. The Config tab shows the merged effective config and lists the source files that contributed.

Every section is optional; with no file anywhere, all defaults apply. Sections: `backend`, `core`, `tinyplace` (identity/presence + peer roster for the daemon and Overview panel), `hub` (the persisted worker roster and selected default worker, so a fleet survives a restart), `stateDir` (default `<home>/state`; `MEDULLA_STATE_DIR` overrides), `opencode` (worker display, model, agent, workspace, concurrency), `memory` (persona-memory switch, roots, identity, model, and the `maxCostUsd` ingest ceiling), `workflow` (the daemon's workspace allowlist), `onboarding` (welcome-flow completion state), `update` (`check = true`/`false` for the background release check; `MEDULLA_NO_UPDATE_CHECK` env kill-switch), `theme` (TUI colors — `primary`/`accent`/`selectionFg`/`dimBorder` as [ratatui](https://ratatui.rs/) color names or `#rrggbb`; the Settings › Appearance subpage edits and persists these), and `medulla.contextWindowTokens` (Context tab usage hint; the orchestration limits section also carries pass/step/depth/task/token bounds). Inference and tracing are server-side concerns — the TUI has no config for them; unknown sections are ignored.

See [`config.example.toml`](https://github.com/tinyhumansai/medulla/blob/main/config.example.toml) for a commented reference and [`src/sdk/src/config.rs`](../../src/sdk/src/config/) for the full schema — fields are camelCase.

## Endpoints

The backend base URL defaults to production, `https://api.tinyhumans.ai`, and the tiny.place endpoint to `https://api.tiny.place`. Set `MEDULLA_STAGING=1` (or `true`, case-insensitive) to switch both defaults to their staging hosts (`https://staging-api.tinyhumans.ai` and `https://staging-api.tiny.place`).

Base-URL precedence, highest first:

* **Backend:** `MEDULLA_API_URL` env var > config-file `backend.baseUrl` > staging/prod default.
* **tiny.place:** the `TINYPLACE_ENDPOINT` / `TINYPLACE_API_URL` / `NEXT_PUBLIC_API_URL` env chain > tinyplace config-file `endpoint` > config-file `tinyplace.baseUrl` > staging/prod default.

Override the base URL (and the token env var name) in the config file — e.g. to point at a local backend:

```json
{
  "backend": {
    "baseUrl": "http://localhost:5000",
    "tokenEnv": "MEDULLA_TOKEN"
  }
}
```

An inline `"token"` field is also accepted, but keep secrets out of committed files — prefer the env var.

## Runtimes

On startup the TUI picks one of three runtimes, in this order. If a preferred runtime fails, it falls back down this chain and shows why in the status line.

1. **Core socket** — if `--core-socket <path>` is passed or the config has a `core` section, and the socket is reachable.
2. **Backend HTTP/SSE** — if a backend token is available (an inline `backend.token`, the `backend.tokenEnv` variable, or credentials saved by [`medulla login`](authentication.md)).
3. **Mock** — otherwise.

In the default (non-`--core-socket`) path, when no token resolves the TUI does not drop straight to the mock: it first opens the [login screen](authentication.md#logging-in-from-the-tui), and the mock runtime is entered only if you press `m` to continue offline. An explicit `--core-socket` run keeps the plain backend→mock fallback and is never redirected to the login screen.

### Mock (zero setup)

```sh
cargo run
```

With no token and no core socket, `medulla` opens a login screen; press `m` to continue offline against the mock runtime — a scripted demo, no credentials, no network, and the fastest way to explore the interface.

### Backend HTTP/SSE

Point the TUI at a running Medulla backend and give it a JWT:

```sh
MEDULLA_TOKEN=<jwt> medulla
```

Or log in through the browser — see [Authentication](authentication.md).

### Core socket

For driving a locally running core orchestration server over its [NDJSON](https://ndjson.org/) Unix-socket protocol:

```sh
medulla --core-socket /path/to/serve.sock
```

The socket path resolves as, highest first: the explicit `--core-socket <path>` flag, then `MEDULLA_CORE_SOCKET`, then `core.socketPath` from the config, then `$XDG_RUNTIME_DIR/medulla/core.sock`, then `<stateDir>/core.sock` (the resolved `stateDir`, which defaults to `<home>/state` and honors `MEDULLA_STATE_DIR`). It is **attach-only** — the TUI connects to an already-running orchestration server and does not launch one. Config form:

```json
{
  "core": { "socketPath": "/tmp/medulla-core.sock" }
}
```

The core runtime unlocks the Routing tab (host and fleet management) and task steering (`X` cancel task, `A` answer a pending question). It is **unix-only** (it rides a Unix domain socket). On Windows a `--core-socket` flag or `[core]` config section resolves to a startup note ("core runtime requires unix sockets — unavailable on Windows") and falls through to the normal backend→mock chain.

## Hosting on this device

A plain `medulla` is both halves of the system: the **orchestrator** that decides
what work to hand out, and a **host** that runs it. The host binds an address on
an in-process bus that the orchestrator dispatches over, so a task for this
machine is delivered in memory — no tiny.place identity, no contact request, no
relay round-trip, and no second `medulla daemon` process beside the TUI. Workers
on other machines still travel over tiny.place; the orchestrator picks per
address, so the two coexist without you configuring anything.

It is on by default and needs no setup. It serves whichever coding-agent CLIs it
finds on `PATH` (`claude`, `codex`, `opencode`), in the directory you launched
from. The Overview tab grows a **This device** panel showing what it will run,
where, and what it has run so far.

```toml
[host]
enabled = true              # false to orchestrate only
address = "this-device"     # local to this process; never goes on a wire
workspace = ""              # empty = the directory you launched from
providers = []              # empty = detect what is installed
defaultProvider = ""        # empty = the first detected
concurrency = 2
taskTimeoutMs = 600000
model = ""
skipPermissions = true
```

`workspace` is the most consequential key here: anything a hosted task edits, it
edits there.

`skipPermissions` defaults to on because a hosted task is unattended — nobody is
in the pane to answer a harness permission prompt, so a task that hits one has
hung until it times out.

### Turning either half off

| Variable | Effect |
| --- | --- |
| `MEDULLA_HOST=0` | Orchestrate only — this machine runs nothing |
| `MEDULLA_HUB=0` | Host only — no orchestrator uplink to the backend |

Both are single-run overrides that beat the config file; `=1` forces the
corresponding half on. Setting both leaves a plain chat client.

If hosting was wanted and could not start — no agent CLI installed, or the
address already bound — the TUI says so on the status line rather than silently
orchestrating into a void.

## Fleet

The `fleet` section declares the capacity Medulla may place work on: the
`Host → Harness → Workspace → Agent` containment chain, plus the agent templates
that may be provisioned onto it. Nothing here is probed — this is what you say
exists.

```json
{
  "fleet": {
    "hosts": [
      {
        "id": "workshop",
        "name": "workshop",
        "availability": "online",
        "resources": { "cpuCores": 10, "availableMemoryBytes": 12884901888 }
      }
    ],
    "harnesses": [
      {
        "id": "workshop-claude",
        "hostId": "workshop",
        "kind": "claude-code",
        "availability": "online",
        "ready": true,
        "providers": ["anthropic"],
        "templateIds": ["implementer"],
        "budgets": [
          {
            "provider": "anthropic",
            "window": "5h",
            "limitTokens": 1000000,
            "remainingTokens": 760000,
            "source": "configured"
          }
        ]
      }
    ],
    "workspaces": [
      {
        "id": "medulla",
        "name": "medulla",
        "path": "/srv/repos/medulla",
        "harnessId": "workshop-claude"
      }
    ],
    "agents": [
      {
        "id": "dev-1",
        "name": "dev-1",
        "description": "Implements scoped changes in the medulla repo.",
        "availability": "online",
        "workspaceId": "medulla",
        "templateId": "implementer"
      }
    ],
    "agentTemplates": [
      {
        "id": "implementer",
        "name": "Implementer",
        "description": "Implements a scoped change and reports what it did.",
        "model": "reasoning"
      }
    ]
  }
}
```

Every level names exactly one parent (`hostId` on a harness, `harnessId` on a
workspace, `workspaceId` on an agent), and an agent with no `workspaceId` is a
local agent, which may name a `hostId` directly instead. `templateIds` on a
harness or workspace narrows the catalog for that place; absent means inherit, so
an allowlist only ever subtracts. A template's optional `harnesses` block holds
per-harness overrides and, by its presence, restricts the harness kinds the
template may run on.

The section is optional. When present it is declared to a local orchestration
server at handshake time, and the Agents rail renders it whenever the runtime
itself reports no capacity — so it is useful even on the mock runtime. A runtime
that *does* report capacity (the hosted backend projects its connected-host
roster onto the same chain) wins; the two are never merged, except that this
client's own template catalog is always merged in, by id, since it is what the
client can offer.

## Agent templates on disk

`agentTemplates` is the only part of `fleet` with a default: a catalog of coding
roles — `plan-writer`, `implementer`, `test-writer`, `code-reviewer`, `debugger`,
`verifier`, `doc-writer`, `refactorer`, `merge-resolver`, `pr-manager`,
`triager`, `repo-orchestrator` — so a fresh install has something to provision.

Those roles are editable. Press `i` on **Routing › Agent Templates** to install
them as one TOML file per role under `~/.medulla/agents/`:

```toml
id = "code-reviewer"
name = "Code Reviewer"
description = "Reviews a change or branch diff and reports what would break."
model = "reasoning"
effort = "high"
tools = ["read", "search", "shell"]
tags = ["code", "review"]

instructions = '''
Review the change you are given against its stated intent…
'''
```

Templates are read from `~/.medulla/agents/*.toml` and then from a project-local
`./.medulla/agents/*.toml`, which overrides the user-global store by `id`. `id`
defaults to the filename, so the smallest useful file is a description and some
instructions. A file that fails to parse costs only itself; the rest of the
catalog still loads.

Precedence, lowest to highest: the built-in catalog, the user-global store, the
project-local store, and `[fleet].agentTemplates` in a config file — which wins
outright, so an explicit empty list opts out of templates entirely. Installing
never overwrites an existing file, and *any* file in a store replaces the
built-in catalog, so a role you delete stays deleted.

Locally registered peers are folded in as hosts on top of whichever of those
applies, since a registered machine *is* the host level of the chain.

### Demo fleet

`MEDULLA_DEMO_FLEET=1`, in the environment or the cwd `.env`, stands in a small
fake fleet — two hosts, three harnesses, two workspaces, two agents, two
templates — so every fleet surface can be exercised with no backend, no socket,
and no registered peer. It is strictly opt-in, and it is the last fallback: any
real capacity, declared or reported, takes precedence over it.
