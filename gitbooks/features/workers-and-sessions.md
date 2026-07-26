---
description: >-
  Workers are the capacity that does the work. Sessions are the thread that
  survives it. The split explains most of how Medulla behaves.
---

# Workers and Sessions

Two words carry most of Medulla's operational model, and they are easy to
conflate because other tools use them loosely. Here they are precise. A worker is
capacity, meaning something that can be handed a task. A session is a
conversation thread, meaning the durable history you return to.

Workers are long-lived and shared across sessions. Sessions accumulate history
and can be resumed, forked, and archived. A single instruction you type runs as
one cycle against a session, and inside that cycle work fans out to workers.

## Workers

A worker is anything the orchestrator can delegate to. In practice that means one
of three things.

A **remote peer** is a machine registered on the fleet, reachable over
[tiny.place](https://tiny.place) and addressable by handle or address. This is
what the TUI's Routing tab manages. A **local harness sandbox** is a configured
`claude-code`, `codex`, or `opencode` instance rooted in a workspace and
published into the roster. A **daemon machine** is `medulla daemon` offering a
machine's installed coding-agent CLIs as one addressable agent.

The common thread is that a worker is a full harness, running with its own
credentials in its own workspace, doing real work. Medulla does not simulate
them.

### Managing them

Fleet management lives on the **Routing** tab, which has five pages: `List
Workers`, `Fleet`, `Add Worker`, `Manage Keys`, and `Strategies`. The list page shows each
registered peer with its handle, label, harness, and — once capacity is
refreshed — its CPU cores and available memory. Press `a` to add a peer, where
the first token is the address or `@handle` and the rest is a label. `Enter` or
`s` selects, `e` edits the label, `d` removes, and a refresh pulls fresh
capacity. A worker carries several identifiers that are deliberately kept
distinct — a stable registry ID for edit/select/remove, a messaging address for
delivery, and a wallet peer ID as identity metadata — and none stands in for
another. The roster persists under the `hub` config section, so a selected
default survives a restart.

Fleet peer management and task steering require a runtime that exposes a worker
surface — the hosted backend wired to a live hub, or the local core runtime —
covered in [Configuration](../developers/configuration.md#runtimes). Without one,
worker management reports itself unavailable rather than silently mutating
unrelated state.

### Seeing the whole fleet

`List Workers` is the peers this hub can dispatch to. The **Fleet** page is one
level up: the declared capacity those dispatches land on, as the containment
chain Medulla actually models.

```
Host  →  Harness  →  Workspace  →  Agent
```

A **host** is a machine, with the cores, memory, and disk it declares. A
**harness** is an agent CLI runtime installed on that host — `claude-code`,
`codex`, `opencode`, `tinyplace`, `openhuman`, or something it names itself —
carrying its providers, its readiness, and its token **budgets**. A **workspace**
is a folder that harness exposes, optionally with a parsed
[`MEDULLA.md`](workspace-profiles.md) profile. An **agent** is a durable identity
deployed into one workspace. Each level names exactly one parent, so an agent's
host is derived by walking up rather than declared twice.

Beside the chain sits the **agent template** catalog: the kinds of agent that may
be provisioned onto it. Where the chain declares *where* an agent can be stood
up, a template declares *what* may be stood up there — a description, default
tools, an abstract model tier, and optional per-harness overrides whose presence
also restricts which harness kinds the template may run on.

The tree is an index; the pane beside it is the full declaration. Select a
harness to read every budget window and seat, a workspace to read its profile, an
agent to see its resolved placement and the template it came from, or a template
to read its instructions. `↑↓`/`jk` browse, `PgUp`/`PgDn` scroll the detail, and
`r` re-reads capacity from the runtime — capacity is declared, never streamed, so
it is pulled rather than pushed.

Nothing is invented and nothing is dropped. A harness whose host is not declared
still renders, under the id it claims, marked as a dangling parent. An agent with
no resolvable placement lands in an `unplaced agents` group rather than being
adopted by an arbitrary host. And an agent with no workspace at all is not a
mistake: that is how a local agent is declared, and the detail pane says so.

Everything on this page is declared rather than probed, which is the point — it
is visible on pass one, for every agent, with no capability round-trip. Against
the hosted backend the connected-worker roster is projected onto the same chain,
so the page is populated even where the manager reports one flat row per worker.
You can also declare a chain yourself in the
[`fleet` config section](../developers/configuration.md#fleet), which the
terminal app declares to a local orchestrator at handshake time and renders when
the runtime reports no capacity of its own.

### Choosing a default worker

The `Strategies` page sets the policy that picks the default worker from the
roster:

| Strategy | Selection rule |
| --- | --- |
| **Manual** | Keep the operator's explicit selection. |
| **Balanced** | Most logical CPU cores, breaking ties by available memory. |
| **CPU First** | Most logical CPU cores. |
| **Memory First** | Most currently available memory. |

Every strategy but Manual operates on capacity a worker actually reported, pulled
with a system-information probe; a worker whose details have not been captured is
not invented into a ranking. See [Orchestrator Routing](routing.md) for how these
sit alongside Medulla's model and harness routing.

### Admitting a peer

A worker is a machine on the open [tiny.place](https://tiny.place) network, so
who may send it work is a real security decision, not a formality. tiny.place
refuses direct messages between peers that are not accepted contacts, which means
accepting a contact is exactly what grants that peer the ability to hand your
machine work. The default posture is manual review: incoming requests collect in
a pending queue for you to accept, decline, or block, and blocking — being
destructive — asks for confirmation. A relay hiccup surfaces as poller health
rather than quietly emptying the queue.

### How work reaches them

The orchestrator does not fan out directly. The reasoning tier delegates, and
tasks are assigned by a legible rule: a task with no explicit target goes to the
least-loaded online worker, spreading a fan-out across distinct idle workers
before doubling up on any one of them.

Two refinements matter in practice. Health is tracked and acted on, so a worker
that has failed repeatedly is marked degraded and skipped while a healthy one is
available. It is not removed, because degraded capacity is still capacity when
nothing else is free. Reuse is also preferred over spawning: before provisioning
new capacity Medulla counts idle workers and says so. It still spawns if asked,
so the check nudges rather than blocks.

Growing the fleet grows the number of addressable workers, not the parallelism
ceiling. Concurrency is a separate, pool-wide cap, covered in
[Token Efficiency and Budgets](token-efficiency.md).

### Failure is a first-class outcome

When a worker fails, Medulla notices and re-delegates. Cancelling one task aborts
exactly that task and leaves its siblings running. A task that genuinely cannot
be recovered is reported as failed rather than papered over. Every task settles
into a definite state, whether done, failed, or cancelled, and none are silently
dropped.

## Sessions

A session is the thread: its message history, the cycles that have run against
it, and enough metadata to find it again. You can create one, list them, resume
one, fork one, or archive it.

Forking is worth calling out. A fork starts a new thread that inherits the
parent's history up to a point, then diverges. When an operation is about to go
one of two ways and you want both, you fork rather than lose the setup.

In the TUI, `/new` starts a fresh session, `/fork [name]` branches the current
one, `/resume` picks an earlier thread, and `/abort` stops the running cycle.
`Ctrl-N` is the shortcut for a new session.

### What survives

Sessions are durable, and how durable depends on how you run Medulla. Against the
backend, sessions persist server-side, so history and the event record replay on
reconnect and a live session streams over SSE. Against a local core runtime,
session state is persisted on disk under the state directory.

Medulla can also run detached from the terminal app, so an operation and its
event log survive the TUI exiting or crashing. Reattaching picks the live session
back up, and more than one terminal can attach to the same session at once and
watch it together.

Separately, `medulla sessions` lists recent local `claude` and `codex` sessions,
read from the harnesses' own directories. Because it reads the source of truth
rather than a mirror, the list is always accurate, and a row resolves back to a
resumable session in its original working directory.

### Steering mid-flight

Sessions are not fire-and-forget. While a fleet is running you can correct the
plan, answer a worker's question with `A`, or cancel a task with `X`, and the
operation absorbs the change rather than restarting. This is what the multi-turn
steering benchmarks in [Benchmarks](../benchmarks.md) measure.

Work can also run detached, so a delegation returns immediately and the operation
continues while you keep going, instead of blocking until the fan-out drains. The
`/async` command toggles the default.

## What you see

The terminal app organizes this into seven tabs: Overview, Chat, Agents, Tasks,
Routing, Memory, and Settings — the last of which holds Usage, Appearance,
Config, Feedback, Trace, Context, Account, and Help, grouped under General,
Debug, and About headings. Overview is the at-a-glance panel: runtime identity
and health, the active cycle, recent events, the last cycle's results, the task
ledger, and any pending decision. The Tasks tab is the planning surface, covered
in [Tasks and Sources](tasks-and-sources.md); Routing is the fleet surface
described above.

The Agents tab is where an operation becomes legible. There is one lane for the
orchestrator and one per agent, idle until its first task and busy while in
flight, with context usage shown per row and, for a placed agent, the host,
harness, workspace, and template it runs as. There is deliberately no lane for
the layer between them: the orchestrator deploys 0..N concurrent managers per
cycle, and what reaches this client is the orchestrator and the agents it is
managing. A manager is how work was fanned out, not something you talk to or
steer — its model calls are still visible verbatim under Settings › Trace. What reaches you is assistant text and short status lines, since
raw tool payloads are filtered out before they hit your screen, on the same
principle that keeps them out of the orchestrator's context.

Worker text itself is never truncated. The filtering removes noise, not content.
