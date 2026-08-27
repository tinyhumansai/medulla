---
description: >-
  The vocabulary of the system: the terms used in the system prompts, the Rust
  types, the wire protocol, and the TUI, with the precise meaning of each.
---

# Glossary

These are the terms used across the Medulla orchestrator. Where a term names both
a concept and a Rust type or a wire field, both are given.

## Orchestrator

The root reasoning model that drives a [cycle](#cycle). It receives user input,
plans the work, delegates to managers, and produces the final reply. The
orchestrator is the only component that talks directly to the user; every other
agent in the system works on its behalf.

## Agent

A declared working identity on a [host](#host): a `harness` type by
[workspace](#workspace) directory, written down in `[fleet].agentDeclarations`
and carrying an `agentId`, an optional name, `roles`, and a workspace `strategy`.
An agent exists because somebody declared it, not because a process happens to be
running. One host runs as many agents as you declare.

The orchestrator delegates [tasks](#task) to agents and lists them in
`agent_list`; each agent has a set of tools, an MCP server inventory, and a
health snapshot (consecutive-ok / consecutive-failed). An agent is idle when it
has no running [sessions](#session) and busy otherwise. A manager manages agents.

## Harness

`harness` is the attribute on an agent that says which coding-assistant CLI its
sessions run: `claude`, `codex`, `opencode`, or a custom preset. It is a value
chosen from a fixed set. It is not an entity in the model, not a level in the
containment chain, and not a noun in the UI, where an operator interacts with an
agent or one of its sessions.

In code it also names the runtime adapter that boots, supervises, and talks to
that CLI:

| Harness type | Transport |
| --- | --- |
| Claude Code | ACP (Agent Client Protocol) over stdio, or legacy JSONL |
| Codex | ACP over stdio |
| `codex-server` | JSON-RPC over stdio to a shared, long-lived `codex app-server` |
| OpenCode | ACP over stdio |

`codex-server` is a [flavor](#provider) of Codex rather than a separate harness
type: it authenticates, bills, and configures as Codex and differs only in that
one process serves every lane instead of one being forked per task. See
[Harness integration](harness-integration.md#codex-on-a-shared-process).

The adapter surfaces a status (idle / running / stopped), a
[task board](harness-integration.md#the-wire-contract) (tracked tasks with status
open, active, blocked, done, cancelled), and an event stream (instruction queued,
cycle start and end, task-board changes). The public wire shapes live in the
`harness_contract` module and are versioned independently of any implementation.

## Host

A machine, local or remote: the environment the agents declared on it run in. A
host is declared (not probed) and carries resource metadata (CPU, memory). It is
the top of the containment chain:

```text
Host → Agent → Session
```

The local host is always present; a remote host is added by tiny.place address
and contributes the agents declared over there. The Hosts tab renders this tree
in full, and its union is what the [hub](#hub) advertises to the backend. The
Sessions tab resolves the same projection but draws only `Host → Session`: the
agent tier decides which lane a session belongs to and where it sorts, and then
gets no row of its own.

The legacy `[fleet]` capacity snapshot still carries an older
`Host → Harness → Workspace → Agent` chain in its own types. That describes
declared capacity, not the entity model above.

## Workspace

A filesystem directory an agent works in, declared as part of that agent together
with its `strategy`: `checkout` (every session of the agent shares the directory,
so they run serially; the v1 default) or `worktree` (a carved per-session copy,
so they run in parallel; a follow-up). A workspace is where agents read, write,
and run code. Each workspace can carry a [`MEDULLA.md`](#medullamd) profile, a
short frontmatter and prose summary that tells the orchestrator what the
directory is and how to route work over it.

## Hub

The central coordination point for outbound task dispatch. When the orchestrator
wants work done, the hub delivers a `TaskRequest` (carrying a `task_id`,
`cycle_id`, `worker_address`, and optional `workflow` identifier) to the target
worker and collects the `TaskOutcome`. It is the outbound half of the daemon's
task loop.

## Cycle

One orchestrator turn: user input, reasoning, tool calls, reply. A cycle begins
when the orchestrator receives a prompt, runs its reasoning loop (which may fan
out tasks and read context), and ends when it emits its final report. The final
message is the only output the caller sees; intermediate tool calls and agent
delegation are internal to the cycle.

## Session

An agent session is one running instance of an agent: what a [task](#task)
actually executes in, and the row the Sessions rail draws. It carries a
`sessionId`, its launch anchor and workspace context, and two facts that are
independent of each other.

Its `origin` is either `orchestrator` (auto-created by a dispatch, labelled from
its task) or `user` (opened from the UI and named by the operator). Origin never
changes. Its `owner` is whoever may drive it right now, and ownership moves:
`ctrl-g` takes a session from the orchestrator, handing it back returns it, and
dispatch skips any session the operator holds.

A task is an agent session; the two differ only by origin. Sessions are never
roster entries; only their control state rides the advert.

The word also names the transport-level conversation the SDK keys by
`(conversation, provider)`. Those come in two orthogonal axes. The class is
either `Bounded` (one turn, a single cycle) or `Unbound` (long-lived, spanning
multiple cycles). The driver is either `Task` frames (request and reply) or
`Envelope` streams (continuous).

A session key is a `(conversation, provider)` pair. Sessions carry configuration
(model, budget, routing), a transcript, and phase tracking.

## Task

A unit of work delegated to an agent. A task is a self-contained instruction with
an optional tool allowlist and a [budget](#budget) (max steps, max tokens). Tasks
run concurrently when fanned out; a dependent chain (A then B then C) stays
inside one task, while independent units become separate tasks. Every task
settles with a status of done, failed, or cancelled, and its result is recorded
in the [ledger](#ledger).

## Ledger

The record of every task ever delegated: its id, instruction, assigned agent,
status, timings, event count, and budget consumption. The ledger is the system's
audit trail: `agent_status` queries it, and task digests surface in the TUI's
Sessions view. It is the source of truth for what happened, regardless of whether
the agent that ran the task is still connected.

## Budget

A resource cap applied to a task or a seat. Task budgets limit `maxSteps` (how
many tool calls an agent may make) and `maxTokens` (how many tokens it may
consume). Seat budgets track provider-level usage windows (tokens consumed
against window capacity, with reset timestamps) and render in the TUI as a
one-line summary, for example `seat Claude Max 5x · 1.2M left`.

## Chunk and environment

The context storage system shared across a cycle. When the orchestrator reads
files, delegates tasks, or receives agent results, the output lands in the
environment as named chunks. The orchestrator pages through it with
`context_list`, `context_search`, `context_peek`, and `context_summarize`. Other
managers working the same cycle share the same environment, so chunks must be
read with care; a chunk another manager wrote may not be yours to interpret.

## Capability probing

Asking an agent what it can actually do before routing work to it. A probe
returns the agent's working directory, accessible directories, git project and
branch, available tools, MCP servers, and provider backends. The result is cached
and shown under the agent in `agent_list`. Probing once per agent lets the
orchestrator match tasks to agents by what they can reach, rather than guessing.

## Deployment

Placing a manager at a specific host and workspace. A deployment is the concrete
instantiation of the fleet's declared containment chain. The orchestrator selects
a host and workspace from the fleet registry, spawns the manager there, and the
manager then picks an agent and begins delegating. Once placed, a deployment is
fixed for the cycle: a manager cannot move to a different host or workspace.

## Workflow

A saved, multi-step directed graph definition, usually acyclic but allowed to
contain bounded loops (see the `loop` node). Each step is a node: triggers, agent
dispatches, transforms, code execution, HTTP requests, and more. An `agent` node
runs as a real agent session on the harness type it names (Claude Code, Codex, or
OpenCode). Workflows are authored as JSON files, stored in layered directories
(personal plus per-repository), run through the vendored `tinyflows` engine, and
surfaced in the TUI's Workflows tab with a canvas, run overlay, and copilot.

## Host link

The transport that carries encrypted task frames between the orchestrator and
workers, specified in [Host link protocol](host-link-protocol.md). Two endpoints
exchange UDP datagrams through a forwarder, served by the same backend as the
rest of the API, which routes bytes it cannot read. It supersedes the tiny.place
mailbox, a store-and-forward relay this codebase no longer talks to. All
coordination end-to-end tests drive it in-process to keep suites deterministic
and offline.

## ACP (Agent Client Protocol)

The [Agent Client Protocol](https://agentclientprotocol.com/) is a versioned,
harness-agnostic wire protocol for coding agents. Medulla uses ACP v1 to talk to
Claude Code, Codex, and OpenCode through one lifecycle and event stream, rather
than teaching the orchestration layer each harness's private JSONL format.

## Runtime

The trait that abstracts what drives the TUI: submit an instruction, read back a
render snapshot, and stream events. [`CloudRuntime`](#cloudruntime) is the
runtime the product ships on; a scripted mock runtime backs `medulla --mock`
and the test suites with no backend at all. `snapshot` and `subscribe` are the
only two methods every surface actually depends on — a `RuntimeSnapshot` is a
fold of an event stream, and that fold does not care where the events came
from.

## CloudRuntime

The [runtime](#runtime) the product ships on. It drives the orchestration API
directly over HTTP through `client::MedullaClient`, with a polled event cursor
standing in for a live feed. Built with `CloudRuntime::new(client)` or
`CloudRuntime::with_hub(client, hub)` when a host also relays a
[hub](#hub) uplink, then configured further with `.with_backend(..)` and
`.with_workspaces(..)`. It replaced a runtime that reached the backend by
asking an embedded OpenHuman core to do it on this SDK's behalf — sessions
were never local state, they live on the backend, so the core added a hop and
nothing else.

## `runtime::cloud::connect` and Readiness

The module that decides whether a real launch gets a working `CloudRuntime`.
`client_from_config` builds a `MedullaClient` from a loaded config and the
process environment, resolving the bearer through the same
`backend.token` → `backend.tokenEnv` → stored-session chain every
backend-facing surface uses. `readiness` answers the question the client alone
cannot: `Ready` (a backend is configured and a token was found), `SignedOut`
(a backend is configured but nothing yields a token), or `Unusable` (no backend
base URL is configured at all) — three states rather than two, because "run",
"sign in", and "stop" each need a different response and collapsing any pair of
them sends an operator down the wrong path.

## Hub plane

The workflow half of the [hub](#hub)'s contract with the Medulla orchestration
backend: `hub::plane::payloads` is the Socket.IO wire shape (`WorkflowDescriptor`
and friends) and `hub::plane::bridge::WorkflowBridge` is the trait an embedding
host implements to answer it — listing its saved workflows, returning one's
detail and run history, and running its authoring copilot. Both used to be
re-exported from the embedded OpenHuman core; with the core gone, Medulla is
the only host left that speaks them, so they are declared here directly instead
of sourced from a desktop product.

## App session and account marker

The app session is Medulla's own record of a signed-in backend connection: a
verified bearer token written to `<root>/<account id>/session.json` by
`auth::session::store`, which checks a candidate token against `/auth/me`
before persisting it so a bad paste fails at `medulla login` rather than on
every later call. Reading it back is resolved through the same precedence as
every other backend-facing call: an inline `backend.token`, then
`backend.tokenEnv` (`MEDULLA_TOKEN` by default), then this stored session.
There is no OS keychain involved. `medulla login` also *adopts* a legacy
`credentials.json` left by an older store — verifying it and rewriting it as a
session rather than discarding it — and only `medulla logout` deletes a stored
session.

The account marker, `active_user.toml` at the root of the Medulla home, is the
one file a process can read before it knows who it is: it names the active
account id, so everything else Medulla persists can be scoped to
`<root>/<user id>`. It is written by the login flow and cleared by logout, the
same layout the embedded OpenHuman core used for its own `~/.openhuman`.

## tinyagents

The vendored agent harness (`vendor/tinyagents`) that runs a [local provider](#local-provider)
turn: the model/tool loop, the provider clients, and the `Tool` trait, but
deliberately not a shell or a filesystem — what a tool is allowed to do depends
on the host's threat model, so a crate that shipped one would be wrong for
every host that disagreed. Medulla supplies those itself (`agent::tools`) and
builds a configured `tinyagents::harness::runtime::AgentHarness` from a route
and a checkout (`agent::harness`). Companion to `tinyflows` (see
[Workflow](#workflow)), the same vendoring pattern for the workflow engine.

## Local provider

The in-process daemon provider (`daemon::providers::local`): the one provider
with no binary behind it. Every other provider is a CLI Medulla resolves,
spawns, and folds JSONL from; this one runs a turn as a bounded model/tool loop
directly in this process, on the vendored `tinyagents` harness plus Medulla's
own tools (`agent::tools`: a path-owning filesystem tool, a supervised shell,
and the containment guard both are rooted at). It replaced the RPC this used to
make into an embedded OpenHuman core, which meant running a model in a loop
with three tools pulled in a whole desktop product — memory engine, channel
providers, cron scheduler — to get them.

Selected by naming the `openhuman` [provider](#provider) — the id string is
unchanged even though nothing is embedded any more. It is never
auto-detected, so a task reaches it only by naming it explicitly. It has no
access to an operator's OpenHuman memory, flows, or credentials (there is no
core left to hold them), no hooks, managed skills, or MCP tools (all three
install onto a *child's* command line, and there is no child here), and no
approval gate — the embedded core used to park external-effect tools for
approval from an unlabelled caller; this harness runs what the model asks for.

## `HarnessProvider::Shell`

A plain interactive shell — `bash`, `zsh`, whatever `$SHELL` names — not a
coding agent. It exists so an operator can open a terminal beside their agents
in the same pane, on the same host, with the same working directory. It is
deliberately excluded from every path that treats providers as interchangeable:
never detected as an available daemon provider, and the one
[`HarnessProvider`](#provider) whose `is_dispatchable()` answers `false`, so a
task frame naming `shell` is refused at the parse before it reaches a host. A
shell reads no prompt and reports no completion, so a dispatched turn to one
would never answer — and a peer able to name one would have a way to run
arbitrary commands on the host with no harness in between.

## Harness wrapper

`Command::Wrapper(HarnessProvider)`, in the TUI's CLI: launching a coding-agent
CLI as a transparent, host-link-bridged wrapper rather than the TUI itself.
Reached by argv0 — running the binary as `claude`, `codex`, or `opencode`
(typically a symlink to the `medulla` binary named after the provider) selects
the matching wrapper instead of the ordinary subcommand parse.

## MEDULLA.md

A workspace profile file at a repository root. It carries a short summary
(roughly 100 to 200 tokens) and optional frontmatter preferences (harnesses,
models, routing hints, file layout). The orchestrator reads it on every cycle to
understand what the directory is and how to decompose work within it. Created
with `medulla init` and registered with `medulla workspace add`. See
[MEDULLA.md](../features/workspace-profiles.md).

## Provider

A coding-assistant CLI: the same axis as an agent's harness type, seen from the
process end. The three supported providers are `claude` (Claude Code), `codex`
(OpenAI Codex), and `opencode`. The daemon spawns the CLI as a subprocess and
communicates over ACP or legacy JSONL.

A provider is chosen together with a transport, and the pair is named by one
word, a flavor. `codex` is Codex on its CLI; `codex-server` is the same provider
on a shared `codex app-server` process. Anything that follows from which vendor
runs the work (credentials, config overrides, inference endpoint, the seat the
tokens bill to) follows from the provider alone, which is why the two are
modelled separately.

## Daemon

A long-running background process (`medulla daemon --headless`) that listens for
inbound task frames from the hub, spawns providers through their harness
adapters, and streams results back. One daemon is one workspace; a fleet is N
daemon processes, not one daemon with N directories.

## TUI

The terminal UI shipped as the `medulla` binary. It renders the fleet, agent
lanes, task transcripts, workflow canvas, settings, and routing, all driven
through the same `Runtime` trait that backs the headless daemon and the mock
demo. See [The TUI](the-tui.md).

## Read next

* [Architecture](architecture.md): how these terms map onto modules.
* [Harness integration](harness-integration.md): the wire contract behind the harness terms.
* [Host link protocol](host-link-protocol.md): the transport spec.
