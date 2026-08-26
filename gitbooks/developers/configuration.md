# Configuration

Medulla reads a layered configuration, persists everything under a single home directory, and selects a runtime at startup.

## Medulla home

Everything Medulla persists lives under one home directory, and that directory
belongs to one account. There are two levels:

* The **root** holds one directory per account, plus the `active_user.toml` marker naming the active one. Nothing else lives there.
  * Default: `~/.medulla`.
  * Local dev: set `MEDULLA_DEV=1` (truthy is `1`/`true`, case-insensitive) and the root becomes `./.medulla` (relative to the cwd; gitignored).
  * Explicit: `MEDULLA_HOME=<path>` overrides both.
* The **home** is `<root>/<account id>`, where config, state, logs, workflows, and the core's own workspace live.

The active account is recorded in `<root>/active_user.toml`, written by
[`medulla login`](authentication.md). Before anyone signs in the account is
`local`, so a signed-out install still has a complete home at `<root>/local`.
`MEDULLA_USER=<id>` selects a different account for one process, ahead of the
marker and without changing it. That is also how you reach the pre-login home
again (`MEDULLA_USER=local`).

`medulla logout` clears the *session* and leaves the marker alone, so subsequent
commands still resolve that account's home. That is deliberate: the account's
`config.toml` is where a staging or self-hosted `backend.baseUrl` lives, and
forgetting which account was active would offer the next login a production
endpoint the operator never configured.

Signing in as a different account moves the marker, never the data: the previous
account's directory stays where it is, and signing back in returns to it. A
running app cannot follow the move; it reports the change and asks for a restart.

An account records the deployment it signed in to in its own `config.toml`, so a
session minted on staging is never later verified against production.

Under the home:

* `session.json`: the app session `medulla login` stores — the verified bearer, the account id, and the `baseUrl` that issued it. Owner-only (`0600`) on unix.
* `config.toml`: the user-global config file.
* `state/`: the default `stateDir`, holding chat history under `chats/`, and workflow run records and engine checkpoints under `state/workflows/runs/` and `state/workflows/checkpoints/`.
* `workflows/*.json`: your [workflow](../features/workflows.md) definitions. A repository's own `<cwd>/.medulla/workflows/*.json` layers on top and shadows a personal one of the same id.
* `link/`: the default host-link identity directory.
* `worker.json`: the [worker profile](cli-reference.md#first-run-worker-registration).

Point `MEDULLA_HOME` at a scratch directory to run against an isolated store, with
its own workflows, agent templates, and state rather than yours. That is what the
test suites and container runs do.

A `.env` file in the current directory is loaded at startup, before anything reads the environment: `KEY=VALUE` lines, `#` comments, an optional `export` prefix, and single/double quotes are stripped. It never overrides variables already set in the process environment. This is the usual way to opt into `MEDULLA_DEV=1` for local dev.

## Layered config

Config is merged from lowest to highest precedence (highest wins):

1. Built-in defaults (production endpoints; `MEDULLA_STAGING` flips the default URLs).
2. User-global `<home>/config.toml`.
3. Project-local `./.medulla/config.toml` (else `./medulla.toml`).
4. Environment variables (`MEDULLA_API_URL`, `MEDULLA_TOKEN` via `tokenEnv`, `MEDULLA_STAGING`, `MEDULLA_STATE_DIR`, and the `MEDULLA_*` harness knobs, whose old `TINYPLACE_*` spelling is deprecated but still read).
5. CLI flags.

Files are merged field-by-field (a recursive table merge), so a project-local file can override just `backend.baseUrl` without discarding the rest of a global file. [TOML](https://toml.io/) is the primary format; `--config <path>` still accepts either `.toml` or `.json` (parser chosen by extension) and bypasses file discovery, but env vars and CLI flags still override it. The Config tab shows the merged effective config and lists the source files that contributed.

### The sections

Every section is optional; with no file anywhere, all defaults apply. Unknown
sections are ignored. Inference and tracing are server-side concerns, and the TUI
has no config for them.

| Section | What it configures |
| --- | --- |
| `backend` | The orchestration backend: base URL, token, and token env var name. |
| `host` | Whether this device also runs the work it orchestrates, and the workspace and roots it advertises. |
| `link` | Host-link identity, forwarder, and the peer roster for the daemon and the Overview panel. |
| `hub` | The persisted worker roster and the selected default worker, so a fleet survives a restart. |
| `stateDir` | Where local state is written. Default `<home>/state`; `MEDULLA_STATE_DIR` overrides. |
| `opencode` | Worker display, model, agent, workspace, and concurrency for the OpenCode provider. |
| `workflow` | The daemon's workspace allowlist, and the workspace roots whose `MEDULLA.md` rides every backend session mint. |
| `fleet` | The declared `Host → Harness → Workspace → Agent` capacity chain and the agent-template catalog. |
| `router` | A custom OpenAI-compatible router the daemon spawns harnesses against. Absent leaves every harness unrouted. |
| `customHarnesses` | Named presets that run a chosen model through Claude Code, Codex, OpenCode, or the in-process local (`openhuman`) harness. |
| `budget` | Operator-declared per-provider budgets. Absent leaves every harness advertising an estimate. |
| `onboarding` | Welcome-flow completion state. |
| `update` | `check = true`/`false` for the background release check. `MEDULLA_NO_UPDATE_CHECK` is the env kill-switch. |
| `theme` | TUI colors: `primary`, `accent`, `selectionFg`, `dimBorder`, and `attention`, as [ratatui](https://ratatui.rs/) color names or `#rrggbb`. `attentionBlink` and `attentionBlinkSeconds` control whether and how quickly attention cues pulse. The Settings › Appearance subpage edits and persists these. |
| `statusLine` | How a harness row on the Sessions rail is laid out. Each of `state`, `harness`, `control`, `thread`, `branch`, and `path` takes a `line1`/`line2`/`line3`/`hidden` placement, a `*When` visibility of `always`/`active`/`alert`, and a `*Style` spelling for where it applies. The Settings › Status line subpage edits these with a live preview, and lists each field's description and the full set of values a row can take. The older `appearance.showHarnessBranch`/`showHarnessPath` booleans are read only when this section is absent. |
| `appearance` | The Sessions sidebar layout, alongside the resource-indicator keys. `sidebarGrouping` of `host`/`path`/`harness`/`none` picks the sidebar's section headers, and `sidebarSort` of `created`/`recent`/`name` orders the agents in a section and the sessions under an agent. The Settings › Appearance subpage edits these live. |
| `medulla.contextWindowTokens` | The Context tab usage hint. The orchestration limits section also carries pass, step, depth, task, and token bounds. |

There is no `memory` section. See
[The TUI](the-tui.md#the-tabs) for what happened to the persona-memory layer.

See [`config.example.toml`](https://github.com/tinyhumansai/medulla/blob/main/config.example.toml) for a commented reference and [`src/sdk/src/config.rs`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/config/) for the full schema. Fields are camelCase.

## Endpoints

The backend base URL defaults to production, `https://api.tinyhumans.ai`. Set `MEDULLA_STAGING=1` (or `true`, case-insensitive) to switch it to `https://staging-api.tinyhumans.ai`.

The link forwarder has no endpoint of its own: it is served by the same backend, so `link.forwarderUrl` defaults to whatever `backend.baseUrl` resolved to and moves with it. Set it explicitly only for a deliberately split deployment.

Base-URL precedence, highest first:

* Backend: `MEDULLA_API_URL` env var, then config-file `backend.baseUrl`, then the staging or production default.
* Link forwarder: config-file `link.forwarderUrl`, then the resolved backend base URL.

Override the base URL (and the token env var name) in the config file, for example to point at a local backend:

```json
{
  "backend": {
    "baseUrl": "http://localhost:5000",
    "tokenEnv": "MEDULLA_TOKEN"
  }
}
```

An inline `"token"` field is also accepted, but keep secrets out of committed files; prefer the env var.

## Runtimes

Two runtimes ship:

1. The embedded OpenHuman core, which is the product runtime. It boots inside the `medulla` process, so there is no server to start, no socket to resolve, no attach handshake to fail, and no unix-only restriction.
2. Mock, a scripted offline runtime for demos and tests, reached with `--mock`.

`--mock` is checked first and skips the token lookup and the login screen entirely, which makes it the only way to get a working runtime with no backend at all. Otherwise the core boots and the TUI runs on it.

A core that boots but has no Medulla backend to talk to (no configured URL, or nobody signed in) takes the offline demo exactly as `--mock` does. This is the documented credential-free start, not a misconfiguration to surface; every drive method would otherwise fail behind a UI that looks live. Before that point the TUI opens the [login screen](authentication.md#logging-in-from-the-tui); press `m` to continue offline.

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

### Upgrading from the external core socket

Versions before the embedded core attached to an external `medulla-serve` NDJSON
Unix socket via `--core-socket`, `MEDULLA_CORE_SOCKET`, or a `[core]` config
section. The core now runs in-process. `medulla run` rejects `--core-socket` with
that explanation instead of absorbing it into the instruction text, and a
`[core]` section left in a config file is inert.

## Hosting on this device

A plain `medulla` is both halves of the system: the **orchestrator** that decides
what work to hand out, and a **host** that runs it. The host binds an address on
an in-process bus that the orchestrator dispatches over, so a task for this
machine is delivered in memory. It needs no host-link identity, no enrollment, no
forwarder round-trip, and no second `medulla daemon` process beside the TUI.
Workers on other machines still travel over the host link; the orchestrator picks
per address, so the two coexist without configuration.

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

`skipPermissions` defaults to on because a hosted task is unattended. Nobody is
in the pane to answer a harness permission prompt, so a task that hits one has
hung until it times out.

### Turning either half off

| Variable | Effect |
| --- | --- |
| `MEDULLA_HOST=0` | Orchestrate only. This machine runs nothing. |
| `MEDULLA_HUB=0` | Host only. No orchestrator uplink to the backend. |

Both are single-run overrides that beat the config file; `=1` forces the
corresponding half on. Setting both leaves a plain chat client.

If hosting was wanted and could not start, because no agent CLI is installed or
the address is already bound, the TUI reports it on the status line.

## Custom harness presets

`customHarnesses` is a list of named presets that run an [OpenRouter](https://openrouter.ai/)
model through one of the coding CLIs. The CLI stays the agent runtime; OpenRouter
supplies the model and the credential. A preset is addressed by its `id`
wherever a harness is named, including a workflow step's `harness` key.

```toml
[[customHarnesses]]
id = "deepseek"
name = "DeepSeek via Claude"
baseHarness = "claude"                # claude, codex, opencode, or openhuman
model = "deepseek/deepseek-chat"
fastModel = "deepseek/deepseek-chat"
hostId = "this-device"                # must match [host].address
default = false
apiKeyEnv = "OPENROUTER_API_KEY"
baseUrl = "https://openrouter.ai/api" # optional; defaults per base harness
contextWindow = 114000                # optional
```

| Field | What it does |
| --- | --- |
| `id` | Required. Letters, numbers, `.`, `-`, and `_` only, because it crosses the fleet protocol. |
| `name` | Required. The operator-facing label. |
| `baseHarness` | Required. `claude`, `codex`, `opencode`, or `openhuman`. |
| `model` | Required. The OpenRouter model id used for the main turn. |
| `fastModel` | Optional. The cheaper model for Claude Code's Sonnet and Haiku tiers. Empty falls back to `model`. |
| `contextWindow` | Optional, in tokens. Claude Code's auto-compaction threshold, and the window a `codexOverrides` preset declares to Codex. |
| `hostId` | Required. The host address that exposes the preset. |
| `default` | Whether a task on that host that names no harness uses this preset. |
| `apiKeyEnv` | The name of the environment variable holding the OpenRouter key. Default `OPENROUTER_API_KEY`. |
| `baseUrl` | Endpoint override. Empty selects `https://openrouter.ai/api` for a Claude base and `https://openrouter.ai/api/v1` for Codex and OpenCode. |
| `codexOverrides` | Codex only, off by default. Spawns with Medulla's Codex config overrides, which changes which account the run authenticates as. |
| `reasoningEffort` | The reasoning effort declared to Codex when `codexOverrides` is on. |

How the model reaches the CLI depends on the base harness. Claude Code takes it
through the model-tier variables, with `model` on the Opus tier and `fastModel`
on the Sonnet, Haiku, and small-fast tiers, so sub-agents stay on OpenRouter too.
Codex and OpenCode take it through their own `-m` argument. OpenHuman takes it as
the `model_override` on the core call — see below.

### Choosing the model an OpenHuman turn runs on

A workflow step may name the harness id `openhuman`, and that turn runs in
Medulla's own process on the embedded core rather than in a spawned CLI. Without
a choice it runs on the core's `default_model`, the cloud alias a turn
self-reports as `Chat V1 (Orchestrator)`. Three routes name a different one, and
they resolve in this order, highest first:

1. `MEDULLA_OPENHUMAN_MODEL` (deprecated spelling: `TINYPLACE_OPENHUMAN_MODEL`)
2. `MEDULLA_HARNESS_MODEL` (deprecated spelling: `TINYPLACE_HARNESS_MODEL`)
3. the step's own `config.model`, or the workflow's `defaults.model`
4. the `[[customHarnesses]]` preset the step selected, through its `model`
5. `medulla workflow run --model <name>`, then `[workflows] defaultModel`
6. nothing — the core's own `default_model` answers

The environment sits on top for the same reason `MEDULLA_<P>_BIN` does: it is the
operator's override of what the configuration says, applied to the machine they
are standing at, with no file to edit. An exported-but-blank value counts as
unset.

### Running an OpenHuman turn on OpenRouter

Naming a model is only half the answer: on its own the name is resolved against
whatever providers the core already has, and one no configured provider serves is
not an error — the agent loop falls through to its resolved default and records
that it skipped the override.

`baseUrl` and `apiKeyEnv` supply the other half, and they are live for
`openhuman` presets. They reach the turn by a different road than they do for a
spawned CLI, which has no child to hand an environment to: Medulla resolves the
key named by `apiKeyEnv`, exchanges it at the loopback attribution proxy for a
machine-local token, and passes the core the mount and that token as a
**per-call** route. The core applies the route to that one turn's in-memory
configuration and never writes it to disk, so pointing a workflow step at
OpenRouter does not repoint the account's own OpenHuman inference — the next turn
without a preset runs exactly where it did before.

The route governs the four roles an agent turn runs on (chat, reasoning, agentic,
coding). Background workloads — memory, embeddings, heartbeat, learning — stay
where the account's configuration puts them, because they run tier-specific
models a coding endpoint generally cannot serve.

So a complete OpenHuman preset needs nothing installed and nothing pre-configured
in the core:

```toml
[[customHarnesses]]
id = "deepseek-oh"
name = "DeepSeek via OpenHuman"
baseHarness = "openhuman"
model = "deepseek/deepseek-chat"
hostId = "this-device"
apiKeyEnv = "OPENROUTER_API_KEY"
```

Routing is skipped, with no error, in three cases: no key exported under
`apiKeyEnv`, a `baseUrl` that resolves somewhere other than `openrouter.ai`, and
a turn with no model resolved. Each leaves the turn on the account's own
OpenHuman configuration, which is why an `openhuman` preset that names only a
model still works and is still advertised as capacity without an OpenRouter key —
unlike every other base harness.

`apiKeyEnv` holds a variable name and never a value. The key stays in the process
environment, and neither the config file nor the app's own state ever holds it. A
host advertises a preset as capacity only when the named variable is set to
something non-blank and the preset's base CLI is one that host runs. The key does
not reach the harness either: an OpenRouter-bound run goes through a loopback
proxy that hands the child a machine-local token instead. See
[Attribution and routing](attribution-and-routing.md).

Presets attach to a host by `hostId`, which must match the `address` of a
`[host]` entry (`this-device` by default). A preset naming another machine is not
advertised or executable here. Declaring one also adds its base CLI to that
host's provider allowlist, since declaring the preset is itself a request to run
that CLI.

The section replaces rather than merges across config layers. A higher-precedence
file that declares `customHarnesses` supplies the whole list; one that does not
declare it leaves the presets from a lower layer in place, so unrelated
project-local settings do not hide them.

Presets are read when a host starts. Adding, editing, or deleting one from
[Hosts › Harness Types](the-tui.md#harness-types) writes the config file
immediately, but the running host keeps the presets it started with, so restart
`medulla` (or the `medulla daemon` on the machine that hosts the preset) before
work can run on a new one.

## Fleet

The `fleet` section declares the capacity Medulla may place work on: the
`Host → Harness → Workspace → Agent` containment chain, plus the agent templates
that may be provisioned onto it. Nothing here is probed; this is what you declare
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
server at handshake time, and the Sessions rail renders it whenever the runtime
itself reports no capacity, so it is useful even on the mock runtime. A runtime
that *does* report capacity (the hosted backend projects its connected-host
roster onto the same chain) wins; the two are never merged, except that this
client's own template catalog is always merged in, by id, since it is what the
client can offer.

## Agent templates on disk

`agentTemplates` is the only part of `fleet` with a default: a catalog of coding
roles (`plan-writer`, `implementer`, `test-writer`, `code-reviewer`, `debugger`,
`verifier`, `doc-writer`, `refactorer`, `merge-resolver`, `pr-manager`,
`triager`, `repo-orchestrator`) so a fresh install has something to provision.

Those roles ship as TOML documents, the same format the store reads, so the
built-in catalog and an installed one are the same files. Press `i` on
Hosts › Agent Templates to copy them into `~/.medulla/agents/`, one file
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
project-local store, and `[fleet].agentTemplates` in a config file, which wins
outright, so an explicit empty list opts out of templates entirely. Installing
never overwrites an existing file, and *any* file in a store replaces the
built-in catalog, so a role you delete stays deleted.

Locally registered peers are folded in as hosts on top of whichever of those
applies, since a registered machine *is* the host level of the chain.

### Demo fleet

`MEDULLA_DEMO_FLEET=1`, in the environment or the cwd `.env`, stands in a small
fake fleet (two hosts, three harnesses, two workspaces, two agents, two
templates) so every fleet surface can be exercised with no backend, no socket,
and no registered peer. It is strictly opt-in, and it is the last fallback: any
real capacity, declared or reported, takes precedence over it.

## Read next

* [The TUI](the-tui.md): the tabs and the screens these settings drive.
* [CLI Reference](cli-reference.md): the subcommands that read this config.
* [Authentication](authentication.md): tokens and the credential store.
