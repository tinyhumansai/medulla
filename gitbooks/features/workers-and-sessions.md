---
description: >-
  Workers are the capacity that does the work. Sessions are the thread that
  survives it. Understanding the split explains most of how Medulla behaves.
---

# Workers and Sessions

Two words carry most of Medulla's operational model, and they are easy to
conflate because other tools use them loosely. Here they are precise:

* A **worker** is capacity — something that can be handed a task.
* A **session** is a conversation thread — the durable history you return to.

Workers are long-lived and shared across sessions. Sessions accumulate history
and can be resumed, forked, and archived. A single instruction you type runs as
one **cycle** against a session, and inside that cycle work fans out to workers.

## Workers

A worker is anything the orchestrator can delegate to. In practice that means one
of three things:

* **A remote peer** — a machine registered on the fleet, reachable over
  [tiny.place](https://tiny.place) and addressable by handle or address. This is
  what the TUI's **Workers** tab manages.
* **A local harness sandbox** — a configured `claude-code`, `codex`, or
  `opencode` instance rooted in a workspace, published into the roster.
* **A daemon machine** — `medulla daemon` offering a machine's installed
  coding-agent CLIs as one addressable agent.

The common thread: a worker is a full harness, running with its own credentials
in its own workspace, doing real work. Medulla does not simulate them.

### Managing them

The Workers tab lists each registered peer with its handle, label, and harness.
`a` adds a peer (first token is the address or `@handle`, the rest is a label),
`Enter` or `s` selects, `e` edits the label, `d` removes. Fleet peer management
and task steering require the core runtime — see
[Configuration › Runtimes](../developers/configuration.md#runtimes).

### How work reaches them

The orchestrator does not fan out directly; the reasoning tier delegates, and
tasks are assigned by a simple, legible rule: **a task with no explicit target
goes to the least-loaded online worker**, spreading a fan-out across distinct
idle workers before doubling up on any one of them.

Two refinements matter in practice:

* **Health is tracked and acted on.** A worker that has failed repeatedly is
  marked degraded and skipped while a healthy one is available. It is not
  removed — degraded capacity is still capacity when nothing else is free.
* **Reuse is preferred over spawning.** Before provisioning new capacity, Medulla
  counts idle workers and says so. It still spawns if asked; the check nudges
  rather than blocks.

Growing the fleet grows the number of addressable workers, not the parallelism
ceiling. Concurrency is a separate, pool-wide cap — see
[Token Efficiency and Budgets](token-efficiency.md).

### Failure is a first-class outcome

When a worker fails, Medulla notices and re-delegates. Cancelling one task aborts
exactly that task and leaves its siblings running. A task that genuinely cannot
be recovered is reported as failed rather than papered over. Every task settles
into a definite state — done, failed, or cancelled — and none are silently
dropped.

## Sessions

A session is the thread: its message history, the cycles that have run against
it, and enough metadata to find it again. You can create one, list them, resume
one, fork one, or archive it.

**Forking** is worth calling out. A fork starts a new thread that inherits the
parent's history up to a point, then diverges. When an operation is about to go
one of two ways and you want both, you fork rather than lose the setup.

In the TUI: `/new` starts a fresh session, `/fork [name]` branches the current
one, `/resume` picks an earlier thread, `/abort` stops the running cycle.
`Ctrl-N` is the shortcut for a new session.

### What survives

Sessions are durable, and how durable depends on how you run Medulla:

* Against the **backend**, sessions persist server-side. History and the event
  record replay on reconnect, and a live session streams over SSE.
* Against a **local core runtime**, session state is persisted on disk under the
  state directory.

Medulla can also run **detached from the terminal app**, so an operation and its
event log survive the TUI exiting or crashing. Reattaching picks the live session
back up — and more than one terminal can attach to the same session at once and
watch it together.

Separately, `medulla sessions` lists recent local `claude` and `codex` sessions,
read from the harnesses' own directories. Because it reads the source of truth
rather than a mirror, the list is always accurate, and a row resolves back to a
resumable session in its original working directory.

### Steering mid-flight

Sessions are not fire-and-forget. While a fleet is running you can correct the
plan, answer a worker's question (`A`), or cancel a task (`X`), and the operation
absorbs the change rather than restarting. This is what the multi-turn steering
benchmarks in [Benchmarks](../benchmarks.md) measure.

Work can also run **detached**: a delegation returns immediately and the
operation continues while you keep going, rather than blocking until the fan-out
drains. `/async` toggles the default.

## What you see

The terminal app organizes this into six tabs: **Overview**, **Chat**,
**Agents**, **Workers**, **Memory**, and **Settings** (which holds Usage,
Appearance, Config, Feedback, Trace, Context, Account, and Help).

The **Agents** tab is where an operation becomes legible: one lane per agent,
idle until its first task and busy while in flight, with context usage shown per
row. What reaches you is assistant text and short status lines — raw tool
payloads are filtered out before they ever hit your screen, on the same principle
that keeps them out of the orchestrator's context.

Worker text itself is never truncated. The filtering removes noise, not content.
