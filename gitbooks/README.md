---
description: >-
  Medulla commands fleets of agent harnesses (Claude Code, Codex, OpenCode) from
  one orchestrator that places the work, streams what every harness is doing,
  and keeps its own reasoning surface small.
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

That differs from pointing a harness at other harnesses in two ways that matter. Every running harness streams its input back as it happens, so what the orchestrator knows about the fleet is current rather than assembled after the fact. And the orchestrator's own reasoning surface stays small, because the bulk of the fleet's output is distilled before it arrives instead of being read into one context window.

## No tmux, no wrapper

Running many agents at once has, until now, meant one of two workarounds. Split the terminal and manage the panes yourself, or wrap the harnesses in another agent and hope it can read everything they produce. Medulla replaces both.

It is one process in one terminal. Opening another agent is `Ctrl-T`: pick a harness or a shell, pick a directory, and it is running. There is no ceiling on how many you keep open — each gets its own PTY and its own live terminal state, maintained in the background whether or not it is the one on screen. A rail lists every one of them; the pane beside it shows whichever you have selected, switching instantly because that session's screen was never stale.

And you are not the one polling. Medulla reads every backgrounded session for the signals that mean it needs a human — a permission prompt, a startup dialog, a blocking error, a bell, a dead session, a finished turn awaiting review — and surfaces them as one mark per row and a count in the title: `⚠ 3 waiting on you`. `Ctrl-]` attaches your keyboard to a session and detaches it again; the rest keep running.

A multiplexer gives you N panes and no opinion about them. Medulla gives you the one that needs you.

## Correctness first, by design

Medulla is built around one principle: get the right answer. When a worker fails, it re-delegates. When results look thin, it verifies. When a task splits, it fans out rather than guessing. Every task settles into a definite state and every budget is enforced, so an operation too large to eyeball still finishes with an answer you can act on.

## Where to go next

* [Why an Orchestrator](why-an-orchestrator-model.md): the failure mode of chat-first orchestration, and what an orchestrator does differently.
* [Context Scaling Without Collapse](rlm-context-scaling.md): how the reasoning surface stays small while the fleet grows.
* [Pricing and Availability](pricing-and-availability.md): pricing, early alpha, and how to request access.

The Features section covers what Medulla does day to day: [workers and sessions](features/workers-and-sessions.md), [workflows](features/workflows.md), [`MEDULLA.md` workspace profiles](features/workspace-profiles.md), [routing](features/routing.md), and [token efficiency and budgets](features/token-efficiency.md).

Building on Medulla? The [Developers](developers/) section covers installing the [TUI](developers/getting-started.md), embedding the [SDK](developers/architecture.md), and wiring your own fleet to the orchestrator.

## What comes next

Models are updated at such a pace that it is easy to forget the harder problem was never any single model's intelligence. It is coordination: making many capable harnesses behave like one coherent operation. Medulla is our first step toward orchestration as a first-class capability.

Fleets with everyone.
