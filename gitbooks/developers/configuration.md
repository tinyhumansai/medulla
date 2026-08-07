# Configuration

Medulla reads a layered configuration, persists everything under a single home directory, and selects a runtime at startup. This page covers all three.

## Medulla home

Everything Medulla persists lives under one home directory, and that directory
belongs to one account. There are two levels:

* The **root** holds one directory per account, plus the `active_user.toml` marker naming the active one. Nothing else lives there.
  * Default: `~/.medulla`.
  * Local dev: set `MEDULLA_DEV=1` (truthy is `1`/`true`, case-insensitive) and the root becomes `./.medulla` (relative to the cwd; gitignored).
  * Explicit: `MEDULLA_HOME=<path>` overrides both.
* The **home** is `<root>/<account id>` — where config, state, logs, workflows, and the core's own workspace live.

The active account is recorded in `<root>/active_user.toml`, written by
[`medulla login`](authentication.md). Before anyone signs in the account is
`local`, so a signed-out install still has a complete home at `<root>/local`.
`MEDULLA_USER=<id>` selects a different account for one process, ahead of the
marker and without changing it — which is also how you reach the pre-login home
again (`MEDULLA_USER=local`).

`medulla logout` clears the *session* and leaves the marker alone, so subsequent
commands still resolve that account's home. That is deliberate: the account's
`config.toml` is where a staging or self-hosted `backend.baseUrl` lives, and
forgetting which account was active would offer the next login a production
endpoint the operator never configured.

Signing in as a different account moves the marker, never the data: the previous
account's directory stays where it is, and signing back in returns to it. A
running app cannot follow the move — it says so and asks for a restart.

An account records the deployment it signed in to in its own `config.toml`, so a
session minted on staging is never later verified against production.

Under the home:

* `workspace/` and `.openhuman/` — the embedded core's state and its config, including the app session `medulla login` stores.
* `config.toml` — the user-global config file.
* `state/` — the default `stateDir`, holding chat history under `chats/`, and workflow run records and engine checkpoints under `state/workflows/runs/` and `state/workflows/checkpoints/`.
* `workflows/*.json` — your [workflow](../features/workflows.md) definitions. A repository's own `<cwd>/.medulla/workflows/*.json` layers on top and shadows a personal one of the same id.
* `link/` — the default host-link identity directory.
* `worker.json` — the [worker profile](cli-reference.md#first-run-worker-registration).

Point `MEDULLA_HOME` at a scratch directory to run against an isolated store — its own workflows, agent templates, and state rather than yours. That is what the test suites and container runs do.

A `.env` file in the current directory is loaded at startup, before anything reads the environment: `KEY=VALUE` lines, `#` comments, an optional `export` prefix, and single/double quotes are stripped. It never overrides variables already set in the process environment — this is the usual way to opt into `MEDULLA_DEV=1` for local dev.

## Layered config

Config is merged from lowest to highest precedence (highest wins):

1. Built-in defaults (production endpoints; `MEDULLA_STAGING` flips the default URLs).
2. User-global `<home>/config.toml`.
3. Project-local `./.medulla/config.toml` (else `./medulla.toml`).
4. Environment variables (`MEDULLA_API_URL`, `MEDULLA_TOKEN` via `tokenEnv`, `MEDULLA_STAGING`, `MEDULLA_STATE_DIR`, the `MEDULLA_*` harness knobs — whose old `TINYPLACE_*` spelling is deprecated but still read).
5. CLI flags.

Files are merged field-by-field (a recursive table merge), so a project-local file can override just `backend.baseUrl` without discarding the rest of a global file. [TOML](https://toml.io/) is the primary format; `--config <path>` still accepts either `.toml` or `.json` (parser chosen by extension) and bypasses file discovery, but env vars and CLI flags still override it. The Config tab shows the merged effective config and lists the source files that contributed.

Every section is optional; with no file anywhere, all defaults apply. Sections: `backend`, `host` (whether this device also runs the work it orchestrates, and the workspace and roots it advertises), `link` (host-link identity, forwarder, and peer roster for the daemon and Overview panel), `hub` (the persisted worker roster and selected default worker, so a fleet survives a restart), `stateDir` (default `<home>/state`; `MEDULLA_STATE_DIR` overrides), `opencode` (worker display, model, agent, workspace, concurrency), `workflow` (the daemon's workspace allowlist, and the workspace roots whose `MEDULLA.md` rides every backend session mint), `fleet` (the declared `Host → Harness → Workspace → Agent` capacity chain and the agent-template catalog), `router` (a custom OpenAI-compatible router the daemon spawns harnesses against; absent leaves every harness unrouted), `budget` (operator-declared per-provider budgets; absent leaves every harness advertising an estimate), `onboarding` (welcome-flow completion state), `update` (`check = true`/`false` for the background release check; `MEDULLA_NO_UPDATE_CHECK` env kill-switch), `theme` (TUI colors — `primary`/`accent`/`selectionFg`/`dimBorder`/`attention` as [ratatui](https://ratatui.rs/) color names or `#rrggbb`, plus `attentionBlink` and `attentionBlinkSeconds` for whether and how fast a cue that needs you pulses; the Settings › Appearance subpage edits and persists these), `statusLine` (how a harness row on the Agents rail is laid out — each of `state`/`harness`/`control`/`thread`/`branch`/`path` takes a `line1`/`line2`/`line3`/`hidden` placement, a `*When` visibility of `always`/`active`/`alert`, and where it applies a `*Style` spelling; the Settings › Status line subpage edits these with a live preview, showing each field's description and the full set of values a row can take, and the older `appearance.showHarnessBranch`/`showHarnessPath` booleans are read only when this section is absent), and `medulla.contextWindowTokens` (Context tab usage hint; the orchestration limits section also carries pass/step/depth/task/token bounds). Inference and tracing are server-side concerns — the TUI has no config for them; unknown sections are ignored.

There is no `memory` section: the persona-memory layer is out of this build, and its config schema went with it.

See [`config.example.toml`](https://github.com/tinyhumansai/medulla/blob/main/config.example.toml) for a commented reference and [`src/sdk/src/config.rs`](../../src/sdk/src/config/) for the full schema — fields are camelCase.

## Endpoints

The backend base URL defaults to production, `https://api.tinyhumans.ai`. Set `MEDULLA_STAGING=1` (or `true`, case-insensitive) to switch it to `https://staging-api.tinyhumans.ai`.

The link forwarder has no endpoint of its own: it is served by the same backend, so `link.forwarderUrl` defaults to whatever `backend.baseUrl` resolved to and moves with it. Set it explicitly only for a deliberately split deployment.

Base-URL precedence, highest first:

* **Backend:** `MEDULLA_API_URL` env var > config-file `backend.baseUrl` > staging/prod default.
* **Link forwarder:** config-file `link.forwarderUrl` > the resolved backend base URL.

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

There are two, and the choice is simple:

1. **The embedded OpenHuman core** — the product runtime. It boots inside the `medulla` process, so there is no server to start, no socket to resolve, no attach handshake to fail, and no unix-only restriction.
2. **Mock** — a scripted offline runtime for demos and tests, reached with `--mock`.

`--mock` is checked first and skips the token lookup and the login screen entirely, which makes it the only way to get a working runtime with no backend at all. Otherwise the core boots and the TUI runs on it.

A core that boots but has no Medulla backend to talk to — no configured URL, or nobody signed in — takes the offline demo exactly as `--mock` does. That is not a misconfiguration to surface; it is the documented credential-free start, and every drive method would otherwise fail behind a UI that looks live. Before that point the TUI opens the [login screen](authentication.md#logging-in-from-the-tui); press `m` to continue offline.

### Mock (zero setup)

```sh
medulla --mock
```

A scripted demo: no credentials, no network, and the fastest way to explore the interface. This is what the test suites drive.

### Signing in

```sh
medulla login          # browser OAuth; stores a verified session
medulla                # runs on the embedded core
```

`MEDULLA_TOKEN=<jwt> medulla` supplies a bearer directly instead. See [Authentication](authentication.md).

> **The core socket is gone.** Earlier versions attached to an external `medulla-serve` NDJSON Unix socket via `--core-socket`, `MEDULLA_CORE_SOCKET`, or a `[core]` config section. The core now runs in-process. The flag is rejected by `medulla run` with that explanation rather than being silently absorbed into the instruction text, and a `[core]` section left in a config file is inert.

## Hosting on this device

A plain `medulla` is both halves of the system: the **orchestrator** that decides
what work to hand out, and a **host** that runs it. The host binds an address on
an in-process bus that the orchestrator dispatches over, so a task for this
machine is delivered in memory — no host-link identity, no enrollment, no
relay round-trip, and no second `medulla daemon` process beside the TUI. Workers
on other machines still travel over the host link; the orchestrator picks per
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

Those roles ship as TOML documents — the same format the store reads — so the
built-in catalog and an installed one are the same files. Press `i` on
**Routing › Agent Templates** to copy them into `~/.medulla/agents/`, one file
per role:

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
