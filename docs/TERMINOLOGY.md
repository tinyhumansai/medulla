# Terminology

This document defines the core terms used across the Medulla orchestrator — the
system prompt vocabulary, the Rust type system, the wire protocol, and the TUI.

---

## Orchestrator

The root reasoning model that drives a **cycle**. It receives user input, plans
the work, delegates to managers, and produces the final reply. The orchestrator is
the only component that talks directly to the user; every other agent in the
system works on its behalf.

## Agent

A **declared** working identity on a **host**: a `harness` type × **workspace**
directory, written down in `[fleet].agentDeclarations` and carrying an `agentId`,
an optional name, `roles`, and a workspace `strategy`. Agents are declared, never
discovered — an agent exists because somebody wrote it down, not because a
process happens to be running. One host runs as many agents as you declare.

The orchestrator delegates **tasks** to agents and lists them in `agent_list`;
each agent has a set of **tools**, an MCP server inventory, and a health snapshot
(consecutive-ok / consecutive-failed). An agent is **idle** when it has no running
**sessions** and **busy** otherwise. A manager _manages_ agents.

## Harness

**A type, not a thing.** `harness` is the attribute on an agent that says which
coding-assistant CLI its sessions run — `claude`, `codex`, `opencode`, or a
custom preset. It is a value in a dropdown; it is never an entity in the model,
never a level in the containment chain, and never a noun in the UI (the thing an
operator interacts with is an **agent** or one of its **sessions**).

In code it also names the runtime adapter that boots, supervises, and talks to
that CLI:

| Harness type   | Transport                                                      |
| -------------- | -------------------------------------------------------------- |
| Claude Code    | ACP (Agent Client Protocol) over stdio, or legacy JSONL         |
| Codex          | ACP over stdio                                                  |
| `codex-server` | JSON-RPC over stdio to a shared, long-lived `codex app-server`  |
| OpenCode       | ACP over stdio                                                  |

`codex-server` is a **flavor** of Codex rather than a separate harness type: it
authenticates, bills, and configures as Codex and differs only in that one
process serves every lane instead of one being forked per task. See
[codex-app-server.md](./codex-app-server.md).

The adapter surfaces a **status** (idle / running / stopped), a **task board**
(tracked tasks with status open → active → blocked → done / cancelled), and an
**event stream** (instruction queued, cycle start/end, task-board changes). The
public wire shapes live in the `harness_contract` module and are versioned
independently of any implementation.

## Host

A machine, local or remote — the environment the agents declared on it run in. A
host is declared (not probed) and carries resource metadata (CPU, memory). It is
the top of the containment chain:

```text
Host → Agent → Session
```

The local host is always present; a remote host is added by tiny.place address
and contributes the agents declared over there. This tree is what both the Agents
tab and the Hosts tab render, and its union is what the hub advertises to the
backend — one projection, rendered twice.

*(The legacy `[fleet]` capacity snapshot still carries an older
`Host → Harness → Workspace → Agent` chain in its own types. That describes
declared capacity, not the entity model above.)*

## Workspace

A filesystem directory an **agent** works in, declared as part of that agent
together with its `strategy`: `checkout` (every session of the agent shares the
directory, so they run serially — the v1 default) or `worktree` (a carved
per-session copy, so they run in parallel — a follow-up). A workspace is where
agents read, write, and run code. Each workspace can carry a `MEDULLA.md`
**profile** — a short frontmatter + prose summary that tells the orchestrator
what the directory _is_ and how to route work over it.

## Hub

The central coordination point for outbound task dispatch. When the orchestrator
wants work done, the **hub** delivers a `TaskRequest` (carrying a `task_id`,
`cycle_id`, `worker_address`, and optional `workflow` identifier) to the target
**worker** and collects the `TaskOutcome`. It is the outbound half of the
daemon's task loop.

## Cycle

One orchestrator turn: **user input → reasoning → tool calls → reply**. A cycle
begins when the orchestrator receives a prompt, runs its reasoning loop (which
may fan out **tasks** and read **context**), and ends when it emits its final
report. The final message is the only output the caller sees — intermediate
tool calls and agent delegation are internal to the cycle.

## Session

**An agent session** is one running instance of an **agent** — what a **task**
actually executes in, and the row under an agent on the Agents rail. It carries a
`sessionId`, its launch anchor and workspace context, and two facts that are
independent of each other:

- **`origin`** — `orchestrator` (auto-created by a dispatch, labelled from its
  task) or `user` (opened from the UI and named by the operator). Origin never
  changes.
- **`owner`** — who may drive it right now. Ownership moves: `ctrl-g` takes a
  session from the orchestrator, handing it back returns it, and dispatch skips
  any session the operator holds.

A task **is** an agent session; the two differ only by origin. Sessions are never
roster entries — only their control state rides the advert.

The word also names the transport-level conversation the SDK keys by
`(conversation × provider)`. Those come in two orthogonal axes:

- **Class:** `Bounded` (one turn — a single cycle) or `Unbound` (long-lived,
  spanning multiple cycles).
- **Driver:** `Task` frames (request/reply) or `Envelope` streams (continuous).

A session key is a `(conversation × provider)` pair. Sessions carry configuration
(model, budget, routing), a transcript, and phase tracking.

## Task

A unit of work delegated to an **agent**. A task is a self-contained instruction
with an optional tool allowlist and a **budget** (max steps / max tokens). Tasks
run concurrently when fanned out; a dependent chain (A → B → C) stays inside one
task, while independent units become separate tasks. Every task settles with a
status — done, failed, or cancelled — and its result is recorded in the **ledger**.

## Ledger

The record of every **task** ever delegated: its id, instruction, assigned agent,
status, timings, event count, and budget consumption. The ledger is the system's
audit trail — `agent_status` queries it, and task digests surface in the TUI's
Agents view. It is the source of truth for what happened, regardless of whether
the agent that ran the task is still connected.

## Budget

A resource cap applied to a **task** or a seat. Task budgets limit `maxSteps`
(how many tool calls an agent may make) and `maxTokens` (how many tokens it may
consume). Seat budgets track provider-level usage windows — tokens consumed vs.
window capacity, with reset timestamps — and render in the TUI as a one-line
summary (e.g. "seat Claude Max 5× · 1.2M left").

## Chunk / Environment

The **context storage system** shared across a **cycle**. When the orchestrator
reads files, delegates tasks, or receives agent results, the output lands in the
environment as named chunks. The orchestrator pages through it with
`context_list`, `context_search`, `context_peek`, and `context_summarize`. Other
managers working the same cycle share the same environment, so chunks must be
read with care — a chunk another manager wrote may not be yours to interpret.

## Capability Probing

Asking an **agent** what it can _actually_ do before routing work to it. A probe
returns the agent's working directory, accessible directories, git project and
branch, available tools, MCP servers, and provider backends. The result is cached
and shown under the agent in `agent_list`. Probing once per agent lets the
orchestrator match tasks to agents by what they can reach, rather than guessing.

## Deployment

Placing a **manager** at a specific **host** + **workspace**. A deployment is
the concrete instantiation of the fleet's declared containment chain. The
orchestrator selects a host and workspace from the fleet registry, spawns the
manager there, and the manager then picks an agent and begins delegating. Once
placed, a deployment is fixed for the cycle — a manager cannot move to a
different host or workspace.

## Workflow

A saved, multi-step **directed graph** definition, usually acyclic but allowed to
contain bounded loops (see the `loop` node). Each step is a
node — triggers, agent dispatches, transforms, code execution, HTTP requests, and
more. An `agent` node runs as a real **agent session** on the harness type it
names (Claude Code, Codex, or OpenCode). Workflows are authored as JSON files, stored in layered directories
(personal + per-repository), run through the vendored `tinyflows` engine, and
surfaced in the TUI's Workflows tab with a canvas, run overlay, and copilot.

## Host link

The transport that carries encrypted task frames between the orchestrator and
workers, specified in [`host-link-protocol.md`](host-link-protocol.md). Two
endpoints exchange UDP datagrams through a **forwarder** — served by the same
backend as the rest of the API — which routes bytes it cannot read. It replaced
the tiny.place mailbox, a store-and-forward relay this codebase no longer talks
to. All coordination e2e tests drive it in-process to keep suites deterministic
and offline.

## ACP (Agent Client Protocol)

The [Agent Client Protocol](https://agentclientprotocol.com/) — a versioned,
harness-agnostic wire protocol for coding agents. Medulla uses ACP v1 to talk to
Claude Code, Codex, and OpenCode through one lifecycle and event stream, rather
than teaching the orchestration layer each harness's private JSONL format.

## MEDULLA.md

A **workspace profile** file at a repository root. It carries a short summary
(~100–200 tokens) and optional frontmatter preferences (harnesses, models,
routing hints, file layout). The orchestrator reads it on every cycle to
understand what the directory _is_ and how to decompose work within it. Created
with `medulla init` and registered with `medulla workspace add`.

## Provider

A coding-assistant CLI — the same axis as an agent's **harness** type, seen from
the process end. The three supported providers are `claude` (Claude Code),
`codex` (OpenAI Codex), and `opencode`. The daemon spawns the CLI as a subprocess
and communicates over ACP or legacy JSONL.

A provider is chosen together with a **transport**, and the pair is named by one
word — a **flavor**. `codex` is Codex on its CLI; `codex-server` is the same
provider on a shared `codex app-server` process. Anything that follows from
*which vendor runs the work* — credentials, config overrides, inference
endpoint, the seat the tokens bill to — follows from the provider alone, which
is why the two are modelled separately.

## Daemon

A long-running background process (`medulla daemon --headless`) that listens for
inbound **task frames** from the **hub**, spawns **providers** through their
harness adapters, and streams results back. One daemon = one **workspace**; a fleet
is N daemon processes, not one daemon with N directories.

## TUI

The terminal UI shipped as the `medulla` binary. It renders the fleet, agent
lanes, task transcripts, workflow canvas, settings, and routing — all driven
through the same `Runtime` trait that backs the headless daemon and the mock
demo.
