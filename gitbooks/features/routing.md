---
description: >-
  Different kinds of thinking cost different amounts. Medulla routes each one to
  a model sized for it, and each task to a harness suited to it.
---

# Orchestrator Routing

A large operation is not one kind of work. Deciding how to decompose a problem,
carrying out a step, and squeezing a verbose transcript into something short are
three genuinely different jobs. Running all three on your most capable model is
how orchestration gets expensive. Running them all on your cheapest is how it
gets wrong.

Medulla splits the work into cognitive tiers and routes each to a model sized for
it.

## The three tiers

| Tier | Job |
| --- | --- |
| **Orchestrator** | Holds the operation. Decides what happens next, reads the distilled picture, funds and reviews delegations. |
| **Reasoning** | Does the thinking inside a step, and owns delegation. This is the tier that actually fans work out. |
| **Compress** | Turns bulk into signal: pass summaries, fleet output, anything verbose enough to crowd a context window. |

Every model call names its tier, and the tier is what gets routed. The
orchestrator tier is deliberately the narrowest surface in the system. It never
reads raw fleet traffic, and it does not even see the reasoning tier's scratch
tools. Keeping it clear is the reason accuracy holds at fleet scale, which
[RLM: Context Scaling Without Collapse](../rlm-context-scaling.md) covers in
detail.

Note the division of labour. The orchestrator does not fan out; the reasoning
tier delegates. The orchestrator decides that work should be decomposed and
reviews what comes back.

## Routing to models

Against the hosted orchestrator, tier-to-model mapping is a server-side concern.
You call one opaque orchestrator SKU, and which model runs each tier underneath
is tuned centrally, including failover to a secondary when a provider degrades.
There is no `model` field on the orchestration surface, and the terminal app has
no configuration for inference. That is intentional rather than an omission.

Running Medulla yourself with your own inference is the other path. You map each
tier to whatever models you like, on any provider. That is what model-agnostic by
design means in
[Open Benchmarks, Open SDKs](../open-benchmarks-open-sdks.md), and every
published benchmark number was produced with off-the-shelf models you can rent
today.

Two softer influences sit on top. A delegated task can carry a preferred model,
which is advisory, so the harness may honour it or fall back to its own
configured model. A [`MEDULLA.md`](workspace-profiles.md) can express preferred
models per tier and preferred harnesses for a repository, also advisory by
design, because routing is a cognitive decision and hard policy belongs to the
host that can enforce it.

## Routing to harnesses

Choosing the model is half of it. The other half is choosing what runs the work:
a `claude-code`, `codex`, or `opencode` instance, rooted in a particular
workspace, with its own permissions and sandboxing.

Each harness is configured with the things that matter operationally, including
which binary, which model, which workspace or pool of workspaces, how many tasks
it runs at once, and its permission or sandbox posture. Medulla normalizes all
three CLIs into one observation model, so a fleet mixing them reads as one
operation instead of three log formats.

Routing here is by resolved agent identity. A task addressed to a specific
configured worker, or auto-assigned to one, reaches that worker. See
[Workers and Sessions](workers-and-sessions.md) for how assignment picks a target
and how degraded capacity is handled.

Transient startup failures are treated as transient. Mass-spawning a pool can
trip a harness's own locking, so Medulla retries those with backoff rather than
failing the task, while surfacing every other failure immediately.

## Runtime selection

Separately from model routing, the terminal app picks how it talks to an
orchestrator at all, falling back down a chain and reporting in the status line
why it did. It tries the core socket first, meaning a locally running
orchestration server. It then tries the backend, the hosted orchestrator, when a
token resolves. Failing both it uses the mock, a scripted offline runtime that
lets you explore with no account.

[Configuration](../developers/configuration.md#runtimes) covers the selection
rules and their edge cases.

## A note on phase-named routing

Medulla does not route by named model per phase. There is no setting that plans
on one model and executes on another.

Phase-appropriate routing is expressed through the tier system above.
Planning-shaped work runs on the reasoning tier, summarization on the compress
tier, and the mapping from tier to concrete model is set once rather than per
phase. What you tune is which kind of thinking goes where, and the mapping stays
consistent across every operation instead of encoding model names into each
workflow.
