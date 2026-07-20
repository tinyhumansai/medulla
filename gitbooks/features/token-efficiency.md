---
description: >-
  Two problems that look alike and aren't: spending as few tokens as possible on
  the orchestrator, and leaving as few as possible unused on seats you already
  paid for.
---

# Token Efficiency and Budgets

Orchestration has two token problems, and they pull in opposite directions.

The first is **spending less**: a model coordinating a hundred harnesses will
drown in their output unless something stops that output from reaching it. The
second is **wasting less**: if you already pay for a Claude Max plan and a Codex
allowance, tokens sitting unused on those seats at the end of the month are money
you spent for nothing.

Medulla treats them as separate problems, because they are.

## Spending less: keeping the surface small

The orchestrator does not read your fleet's traffic. Bulk output goes into an
addressable store that it queries deliberately, and what stays in front of the
model is pointer-sized. This is the RLM idea applied to a live fleet, and it is
why the reasoning surface stays small no matter how large the operation grows —
see [RLM: Context Scaling Without Collapse](../rlm-context-scaling.md).

Several mechanisms enforce it:

* **Offload by threshold.** A tool result past a size limit moves to the store;
  the transcript keeps a pointer and a head excerpt.
* **Compressed history.** Each reasoning pass leaves a summary rather than a
  transcript — roughly 20:1 — so a long operation carries forward without
  carrying everything.
* **A gate on fleet output.** Worker events are filtered and compressed, or
  dropped, before reaching a thinking layer — which is to say, before they cost
  anything.
* **Debounced worker events.** A chatty harness cannot churn the orchestrator's
  prompt cache by emitting constantly.
* **Narrowed tools.** A delegated task can bind a subset of tools instead of the
  whole registry, which cuts per-call input tokens sharply on a wide fan-out.
* **A context guard.** When utilization crosses a high-water mark, older material
  is evicted to the store rather than left to crowd the window.

The payoff is measurable: Medulla's native workers average around 6,000 tokens
per task, where an equivalent full harness session runs about 16 times that. That
per-worker efficiency is what makes thousand-harness fleets economically sane.

It also changes what you pay for. Because only the distilled slice reaches the
orchestrator, you pay orchestrator rates on that slice — not on the millions of
tokens moving through your fleet. Cached input is metered and priced separately;
see [Pricing and Availability](../pricing-and-availability.md).

## Budgets, and what happens at the ceiling

Budgets in Medulla are enforced, not advisory, and they operate at several
scopes:

| Scope | What it bounds |
| --- | --- |
| **Cycle** | Total token draw for one instruction, plus a deadline and a concurrency cap. |
| **Task** | A worker's step count and token allowance, sized by the orchestrator to the task. |
| **Depth** | How many levels deep delegation may recurse. |
| **Account** | A daily spend limit across everything. |

Two design decisions make these behave well under pressure.

**Concurrency is a semaphore, not a scheduler.** Excess tasks queue and run as
slots free up. Nothing is rejected for arriving at a busy moment — a two-hundred
task fan-out completes under a cap of eight. This is why "no task is ever
silently dropped" is a claim Medulla can actually make.

**Exhaustion is reported in-band.** When a budget runs out, the model receives an
error it can reason about and recover from, rather than an exception that kills
the operation. A cycle always produces a reply. Termination is guaranteed by
construction — pass ceilings, a forced final turn, the budget gate, and the depth
cap — so an operation cannot spin indefinitely.

The daily spend limit is charge-then-block: a single in-flight call may push you
slightly past the cap, and subsequent calls are refused once cumulative spend
reaches it.

## Wasting less: tokenmax

Here is the second problem, and it is the one most tooling ignores entirely.

If you have connected your own paid subscriptions — a Claude Max plan, a ChatGPT
Pro seat, a Codex allowance — those tokens are already bought. Your harnesses
burn them with your own credentials on your own machines. Leaving them unused at
the end of a window is not saving money; it is throwing money away.

So Medulla steers delegation toward seats that still have headroom. Concretely:

* Workers on a seat with room **sort first**, and the fullest seat drains first.
* A seat with too little headroom to be worth a task drops to cooldown and is
  skipped by automatic assignment.
* Each seat's remaining headroom is written into what the orchestrator reads, so
  it can size a task's token budget to a seat that actually fits it.
* Usage is metered back to the seat it was drawn from.

Two properties keep this honest. **Tokenmax is a preference, never a block** — an
explicitly targeted worker still runs even on an exhausted seat. And the
accounting **fails open**: if seat information is unavailable, delegation proceeds
normally rather than stalling. Budget accounting is soft by contract, because an
orchestrator that halts over its own bookkeeping is worse than one that
occasionally over-delegates.

Published allowances are estimates — providers do not publish exact per-window
numbers — so Medulla treats its own ceilings as a starting guess and corrects
them against what providers actually do. A seat that gets throttled earlier than
predicted is backed off regardless of what the estimate said.

Seats can be prioritized, parked without disconnecting, or pinned to specific
workers. Cost figures attached to a plan are reporting only — how much value you
extracted from something you already bought. They are never billed.

## Seeing where it went

Settings › **Usage** (or `/usage`) shows this session's spend broken out by tier,
a sub-agent row with its task count, and per-task detail underneath — plus
account totals when you are logged in: plan, spend and call count for the cycle,
remaining balance, and a per-model breakdown.

The Agents tab carries the live view: context used against the window, per row,
with a bar for the selected agent. Watching that fill is usually how you notice a
task is shaped wrong.
