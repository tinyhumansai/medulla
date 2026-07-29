---
description: >-
  Medulla commands fleets of agent harnesses — Claude Code, Codex, OpenCode —
  from one orchestrator that places the work, streams what every harness is
  doing, and keeps its own reasoning surface small.
cover: .gitbook/assets/screen.png
coverY: 356.141681768691
coverHeight: 417
layout:
  width: default
  cover:
    visible: true
    size: full
    mask: none
  title:
    visible: true
  description:
    visible: true
  tableOfContents:
    visible: true
  outline:
    visible: true
  pagination:
    visible: true
  metadata:
    visible: true
  tags:
    visible: true
  actions:
    visible: true
---

# Medulla - The Orchestrator

Medulla commands fleets of agent harnesses. Instead of driving [Claude Code](https://www.anthropic.com/claude-code), [Codex](https://github.com/openai/codex), or [OpenCode](https://github.com/sst/opencode) one terminal at a time, you run one orchestrator that decides what work to hand out, places it on a harness that can do it, and keeps a live picture of everything running underneath.

Two things make that different from pointing a harness at other harnesses:

1. **Live streaming input from every running harness**, so fleet awareness is continuous rather than post-hoc.
2. **A reasoning surface that stays small**, because the bulk of the fleet's output is distilled before it reaches the orchestrator rather than being read into one context window.

## Correctness First, by Design

Medulla is built around one principle: get the right answer. When a worker fails, it re-delegates. When results look thin, it verifies. When a task splits, it fans out rather than guessing. Every task settles into a definite state, and every budget is enforced, so an operation too large to eyeball still ends in an answer rather than a shrug.

## Where to Go Next

* [Why an Orchestrator](why-an-orchestrator-model.md) — the failure mode of chat-first orchestration, and what an orchestrator does differently.
* [Context Scaling Without Collapse](rlm-context-scaling.md) — how the reasoning surface stays small while the fleet grows.
* [Pricing and Availability](pricing-and-availability.md) — pricing, early alpha, and how to request access.

The Features section covers what Medulla does day to day: [workers and sessions](features/workers-and-sessions.md), [tasks and sources](features/tasks-and-sources.md), [workflows](features/workflows.md), [`MEDULLA.md` workspace profiles](features/workspace-profiles.md), [routing](features/routing.md), and [token efficiency and budgets](features/token-efficiency.md).

Building on Medulla? The [Developers](developers/) section covers installing the [TUI](developers/getting-started.md), embedding the [SDK](developers/architecture.md), and wiring your own fleet to the orchestrator.

## What Comes Next

Models are updated at such a pace that it is easy to forget the harder problem was never any single model's intelligence. It is coordination: making many capable harnesses behave like one coherent operation. Medulla is our first step toward orchestration as a first-class capability.

Fleets with everyone.
