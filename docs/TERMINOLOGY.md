# Terminology

This document defines the core terms used across Medulla: the Rust type
system, the wire protocol, and the TUI. Medulla is a terminal UI for running
coding agents across several surfaces, remote machines, and instances. There is
no reasoning model above the agents: every session is driven by the operator,
by a workflow, or by another agent through the local hub.

---

## Session

One running process on its own pseudo-terminal: a coding agent in a directory,
or a shell. A session has a row in the rail, a screen kept live in the
background, and a state (in flight, idle, waiting on you, failed, finished).
Sessions exist on this machine and on remote hosts, and the rail shows both.

A session carries a `sessionId`, its launch anchor and workspace context, and
two facts that are independent of each other. Its `origin` is either `task`
(created by a dispatch, labelled from its task) or `user` (opened from the UI
and named by the operator). Origin never changes. Its `owner` is whoever may
drive it right now, and ownership moves: `ctrl-g` takes a dispatched session
over, handing it back returns it, and dispatch skips any session the operator
holds.

The word also names the transport-level conversation the SDK keys by
`(conversation × provider)`. The class is either `Bounded` (one turn) or
`Unbound` (long-lived, spanning many turns). The driver is either `Task` frames
(request/reply) or `Envelope` streams (continuous).

## Agent

A **declared** working identity on a **host**: a `harness` type × **workspace**
directory, written down in `[fleet].agentDeclarations` and carrying an `agentId`,
an optional name, `roles`, and a workspace `strategy`. An agent exists because
somebody declared it, not because a process happens to be running. One host runs
as many agents as you declare.

Declaring one is a config-only operation: `[fleet].agentDeclarations` is edited
by hand, and the TUI reports the resulting tree on the Hosts tab. An agent is
**idle** when it has no running **sessions** and **busy** otherwise.

## Harness

`harness` is the attribute on an agent that says which coding-assistant CLI its
sessions run: `claude`, `codex`, `opencode`, or a custom preset. It is a value
chosen from a fixed set. It is not an entity in the model, not a level in the
containment chain, and not a noun in the UI, where an operator interacts with an
**agent** or one of its **sessions**.

In code it also names the runtime adapter that boots, supervises, and talks to
that CLI:

| Harness type   | Transport                                                      |
| -------------- | -------------------------------------------------------------- |
| Claude Code    | ACP (Agent Client Protocol) over stdio, or legacy JSONL         |
| Codex          | ACP over stdio                                                 |
| `codex-server` | JSON-RPC over stdio to a shared, long-lived `codex app-server`  |
| OpenCode       | ACP over stdio                                                 |
| OpenHuman      | In-process: no binary spawned, no transport. The wire value is `HarnessProvider::Openhuman`; the agent turn runs inside the `medulla` process on the vendored `tinyagents` crate with Medulla's own tools. Never auto-selected by provider detection — a node reaches it only by naming it. |
| Shell          | None: a plain interactive shell (`bash`, `zsh`, whatever `$SHELL` names), not a coding agent. Never detected as an available provider and never dispatchable; it exists so an operator can open a terminal beside their agents in the same pane, host, and working directory. |

`codex-server` is a **flavor** of Codex rather than a separate harness type: it
authenticates, bills, and configures as Codex and differs only in that one
process serves every lane instead of one being forked per task. See
[codex-app-server.md](./codex-app-server.md).

The adapter surfaces a **status** (idle / running / stopped), a **task board**
(tracked tasks with status open → active → blocked → done / cancelled), and an
**event stream** (instruction queued, turn start/end, task-board changes). The
public wire shapes live in the `harness_contract` module and are versioned
independently of any implementation.

## Host

A machine, local or remote: the environment the agents declared on it run in. A
host is declared (not probed) and carries resource metadata (CPU, memory). It is
the top of the containment chain:

```text
Host → Agent → Session
```

The local host is always present; a remote host is a `[[remoteHosts]]` entry
reached over SSH for the bootstrap and then directly over UDP, or a paired
host started with `medulla daemon <key>`. The Hosts tab renders this tree in
full. The Sessions tab resolves the same projection but draws only
`Host → Session`: the agent tier decides which lane a session belongs to and
where it sorts, and then gets no row of its own.

## Workspace

A filesystem directory an **agent** works in, declared as part of that agent
together with its `strategy`: `checkout` (every session of the agent shares the
directory, so they run serially; the default) or `worktree` (a carved
per-session copy, so they run in parallel). A workspace is where agents read,
write, and run code, and it is the third step of the session picker.

## Hub

The device-local coordination point for task dispatch. When a workflow step, an
MCP `fleet_dispatch` call, or the operator asks for work to be run somewhere,
the **hub** delivers a `TaskRequest` (carrying a `task_id`, `worker_address`,
and optional `workflow` identifier) to the target **worker** and collects the
`TaskOutcome`. It keeps the worker roster (`[hub].workers`), the activity log,
and the `TaskRunner` that owns dispatch timeouts. It runs inside the `medulla`
process and talks to nothing outside the machine except the workers it
dispatches to.

## Task

A unit of work delegated to an **agent**: a self-contained instruction with an
optional tool allowlist and a **budget** (max steps / max tokens). Tasks run
concurrently when fanned out; a dependent chain (A → B → C) stays inside one
task, while independent units become separate tasks. Every task settles with a
status of done, failed, or cancelled. A task **is** an agent session; the two
differ only by origin.

## Budget

A resource cap applied to a **task** or a seat. Task budgets limit `maxSteps`
(how many tool calls an agent may make) and `maxTokens` (how many tokens it may
consume). Seat budgets track provider-level usage windows (tokens consumed
against window capacity, with reset timestamps) and render in the TUI as a
one-line summary (e.g. "seat Claude Max 5× · 1.2M left").

## Capability probing

Asking an **agent** what it can _actually_ do before routing work to it. A probe
returns the agent's working directory, accessible directories, git project and
branch, available tools, MCP servers, and provider backends. The result is cached
and shown under the agent on the Hosts tab, so a workflow or an operator can
match tasks to agents by what they can reach rather than guessing.

## Workflow

A saved, multi-step **directed graph** definition, usually acyclic but allowed to
contain bounded loops (see the `loop` node). Each step is a node: triggers, agent
dispatches, transforms, code execution, HTTP requests, and more. An `agent` node
runs as a real **agent session** on the harness type it names (Claude Code,
Codex, or OpenCode). Workflows are authored as JSON files, stored in layered
directories (personal + per-repository), run through the vendored `tinyflows`
engine, and surfaced in the TUI's Workflows tab with a canvas, run overlay, and
copilot. See [workflows.md](./workflows.md).

## Host link

The transport that carries encrypted frames between a Medulla and its remote
hosts, specified in [`host-link-protocol.md`](host-link-protocol.md). Two
endpoints exchange UDP datagrams straight to each other under a **pair key**
that only they hold; there is no relay in the production path. Offline
coordination e2e suites instead use an in-process relayed route, keeping those
suites deterministic and offline; the direct path is covered separately by the
live harness.

## Pair key and host key

The **pair key** is the 128-bit secret the two ends of a host link share. On the
SSH-bootstrapped path it is minted by `medulla daemon --direct` and carried back
once inside the SSH channel. On the paired path it rides inside the **host
key**, a single `HK1-…` string minted on the client that also names both node
ids and the UDP port, and is pasted once into `medulla daemon <key>`. Either
way it is stored only in `<home>/link/node.json` and never in config.

## ACP (Agent Client Protocol)

The [Agent Client Protocol](https://agentclientprotocol.com/) is a versioned,
harness-agnostic wire protocol for coding agents. Medulla uses ACP v1 to talk to
Claude Code, Codex, and OpenCode through one lifecycle and event stream, rather
than teaching the session layer each harness's private JSONL format. Because
Medulla is the client, the only way to hand a harness tools is to offer it an
MCP server, which is what `medulla mcp` is for.

## Provider

A coding-assistant CLI: the same axis as an agent's **harness** type, seen from
the process end. The three coding-CLI providers are `claude` (Claude Code),
`codex` (OpenAI Codex), and `opencode`. The daemon spawns the CLI as a subprocess
and communicates over ACP or legacy JSONL.

Two more providers sit outside that coding-CLI set: `openhuman`, the in-process
harness that runs an agent turn inside the `medulla` process instead of
spawning one (see **Harness** above), and `shell`, a plain interactive shell
that is never dispatchable or auto-detected.

A provider is chosen together with a **transport**, and the pair is named by one
word, a **flavor**. `codex` is Codex on its CLI; `codex-server` is the same
provider on a shared `codex app-server` process. Anything that follows from
*which vendor runs the work* (credentials, config overrides, inference endpoint,
the seat the tokens bill to) follows from the provider alone, which is why the
two are modelled separately.

## Daemon

`medulla daemon`, the process that serves sessions on a machine to a Medulla
elsewhere. `medulla daemon --direct` is the variant the SSH bootstrap starts on
a remote host; `medulla daemon <key>` is the paired variant the operator starts
once from a host key. Both serve the one client that holds the pair key, over
UDP.

## Backend

The TinyHumans API at `api.tinyhumans.ai`. Signing in and plan entitlement
(`/auth/me`, which decides whether the account may run Medulla) are separate
account flows. The SDK's `Backend` trait (`backend` module) covers account usage,
logout, and the feedback board. No session, task, transcript, or workspace
content goes to it. `medulla --mock` swaps in a scripted `MockBackend`, and a
signed-out run uses an `OfflineBackend`.

## TUI

The terminal UI shipped as the `medulla` binary. It renders the session rail,
agent lanes, transcripts, workflow canvas, hosts, and settings. It holds a
`Backend` for the account-side calls and a hub slot for local dispatch, and
everything else is local state.
