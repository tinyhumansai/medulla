---
description: >-
  Different kinds of thinking cost different amounts. Medulla routes each one to
  a model sized for it, and each task to a harness suited to it.
---

# Orchestrator Routing

A large operation is not one kind of work. Deciding how to decompose a problem,
carrying out a step, and squeezing a verbose transcript into something short are
three genuinely different jobs. Put all three on your most capable model and you
pay top-tier rates for summarization. Put them all on your cheapest and the
decomposition suffers.

Medulla splits the work into cognitive tiers and routes each to a model sized for
it.

## The three tiers

| Tier | Job |
| --- | --- |
| **Orchestrator** | Holds the operation. Decides what happens next, reads the distilled picture, funds and reviews delegations. |
| **Reasoning** | Does the thinking inside a step, and owns delegation. This is the tier that actually fans work out, as 0..N concurrent **managers** per cycle, each deployed by the orchestrator, each choosing its own harness and agents. |
| **Compress** | Turns bulk into signal: pass summaries, fleet output, anything verbose enough to crowd a context window. |

Every model call names its tier, and the tier is what gets routed. The
orchestrator tier is deliberately the narrowest surface in the system. It never
reads raw fleet traffic, and it does not even see the reasoning tier's scratch
tools. Keeping it clear is the reason accuracy holds at fleet scale, which
[RLM: Context Scaling Without Collapse](../rlm-context-scaling.md) covers in
detail.

Note the division of labour. The orchestrator does not fan out; the reasoning
tier delegates. The orchestrator decides that work should be decomposed and
reviews what comes back. Concretely: the orchestrator chooses *where* a manager
works (the host and workspace, frozen for that manager's whole run), and the
manager chooses *who* does the work and *on what runtime*, selecting the harness,
targeting or spawning agents, and delegating tasks to them.

Managers are not shown in the terminal app. The event stream carries the
orchestrator and the agents it is managing; a manager's thinking and intermediate
tool calls never ride upward, only the message from its last turn. See
[Workers and Sessions](workers-and-sessions.md#what-you-see).

## Routing to models

Against the hosted orchestrator, tier-to-model mapping is a server-side concern.
You call one opaque orchestrator SKU, and which model runs each tier underneath
is tuned centrally, including failover to a secondary when a provider degrades.
The orchestration surface therefore has no `model` field, and the terminal app
has no configuration for inference.

Running Medulla yourself with your own inference is the other path. You map each
tier to whatever models you like, on any provider; nothing in the tier system
depends on a specific model or vendor.

Above that mapping sit two advisory inputs. A delegated task can carry a
preferred model, which the harness may honour or ignore in favour of its own
configured model. A [`MEDULLA.md`](workspace-profiles.md) can express preferred
models per tier and preferred harnesses for a repository. Both stay advisory
because routing is a cognitive decision, and hard policy belongs to the host that
can enforce it.

## Routing to harnesses

The model is one decision; what runs the work is the other. That means a
`claude-code`, `codex`, or `opencode` instance, rooted in a particular workspace,
with its own permissions and sandboxing.

Each harness is configured with the things that matter operationally, including
which binary, which model, which workspace or pool of workspaces, how many tasks
it runs at once, and its permission or sandbox posture. Medulla normalizes all
three CLIs into one observation model, so a fleet mixing them reads as one
operation instead of three log formats.

A harness can also be a named preset that runs an
[OpenRouter](https://openrouter.ai/) model through one of those three CLIs. The
CLI is still what executes the work; the preset only changes where the model
comes from, so a task or a workflow step asks for `deepseek` the same way it asks
for `codex`. Presets are declared per host, and a host offers one only when it
has the OpenRouter key that preset names. See
[Custom harness presets](../developers/configuration.md#custom-harness-presets)
for the fields and
[Attribution and routing](../developers/attribution-and-routing.md) for how the
key is kept out of the harness itself.

Routing here is by resolved agent identity. A task addressed to a specific
configured worker, or auto-assigned to one, reaches that worker. When no explicit
target is named, a default-worker strategy (Manual, Balanced, CPU First, or
Memory First) picks from the roster using capacity each worker actually reported,
so a fan-out spreads across real cores and free memory rather than a guess. See [Workers and Sessions](workers-and-sessions.md) for the strategies in
full, how assignment picks a target, and how degraded capacity is handled.

Transient startup failures are treated as transient. Mass-spawning a pool can
trip a harness's own locking, so Medulla retries those with backoff rather than
failing the task, while surfacing every other failure immediately.

## Runtime selection

Separately from model routing, the terminal app picks how it talks to an
orchestrator at all, and reports in the status line why. It runs on an OpenHuman
core embedded in the same process, so there is no server to reach and nothing to
attach to. `--mock` selects a scripted offline runtime instead, and a core that
boots with nobody signed in takes that same offline runtime rather than pretending
to be live.

[Configuration](../developers/configuration.md#runtimes) covers the selection
rules and their edge cases.

## Routing by phase

Phase-appropriate routing goes through the tiers. Planning-shaped work runs on
the reasoning tier and summarization on the compress tier, and the mapping from
tier to concrete model is set once, not per phase: there is no setting that names
one model for planning and another for execution. What you tune is which kind of
thinking goes where, so the mapping stays consistent across every operation
instead of model names being encoded into each workflow.
