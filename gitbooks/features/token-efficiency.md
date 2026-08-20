---
description: >-
  Two problems that look alike and are not: spending as few tokens as possible on
  the orchestrator, and leaving as few as possible unused on seats you already
  paid for.
---

# Token Efficiency and Budgets

Orchestration has two token problems, and they pull in opposite directions.

The first is spending less. A model coordinating a hundred harnesses will drown
in their output unless something stops that output from reaching it. The second
is wasting less. If you already pay for a Claude Max plan and a Codex allowance,
tokens sitting unused on those seats at the end of the month are money you spent
for nothing.

Medulla handles the two separately, and the rest of this page is split the same
way.

## Spending less: keeping the surface small

The orchestrator does not read your fleet's traffic. Bulk output goes into an
addressable store that it queries deliberately, and what stays in front of the
model is pointer-sized. This is the RLM idea applied to a live fleet, and it is
why the reasoning surface stays small no matter how large the operation grows.
See [RLM: Context Scaling Without Collapse](../rlm-context-scaling.md).

Several mechanisms enforce it.

* A tool result past a size limit is offloaded to the store, and the transcript
  keeps a pointer and a head excerpt.
* Each reasoning pass leaves a summary rather than a transcript, roughly 20:1, so
  a long operation carries forward without carrying everything.
* Worker events are filtered and compressed, or dropped, before reaching a
  thinking layer, which is to say before they cost anything.
* Those events are also debounced, so a chatty harness cannot churn the
  orchestrator's prompt cache by emitting constantly.
* A delegated task can bind a subset of tools instead of the whole registry,
  cutting per-call input tokens sharply on a wide fan-out.
* When context utilization crosses a high-water mark, a guard evicts older
  material to the store rather than leaving it to crowd the window.

Our own measurements put Medulla's native workers at around 6,000 tokens per
task, against roughly 16 times that for an equivalent full harness session. That
is an internal figure rather than a benchmark, but it is the per-worker
arithmetic that decides whether a fleet of a thousand harnesses is affordable.

It also changes what you pay for. Because only the distilled slice reaches the
orchestrator, you pay orchestrator rates on that slice rather than on the
millions of tokens moving through your fleet. Cached input is metered and priced
separately, covered in
[Pricing and Availability](../pricing-and-availability.md).

## Budgets, and what happens at the ceiling

Budgets in Medulla are enforced rather than advisory, and they operate at several
scopes.

| Scope | What it bounds |
| --- | --- |
| **Cycle** | Total token draw for one instruction, plus a deadline and a concurrency cap. |
| **Task** | A worker's step count and token allowance, sized by the orchestrator to the task. |
| **Depth** | How many levels deep delegation may recurse. |
| **Account** | A daily spend limit across everything. |

How they behave when they bind is the part that matters.

Concurrency is a semaphore rather than a scheduler. Excess tasks queue and run as
slots free up, and nothing is rejected for arriving at a busy moment, so a
two-hundred task fan-out completes under a cap of eight. This is why Medulla can
claim that no task is ever silently dropped.

Exhaustion is reported in-band. When a budget runs out, the model receives an
error it can reason about and recover from, instead of an exception that kills
the operation. A cycle always produces a reply. Termination is guaranteed by
construction, through pass ceilings, a forced final turn, the budget gate, and
the depth cap, so an operation cannot spin indefinitely.

The daily spend limit works by charging then blocking. A single in-flight call
may push you slightly past the cap, and subsequent calls are refused once
cumulative spend reaches it.

## Wasting less: tokenmax

If you have connected your own paid subscriptions, whether a Claude Max plan, a
ChatGPT Pro seat, or a Codex allowance, those tokens are already bought. Your
harnesses burn them with your own credentials on your own machines. Tokens left
unused when the window resets are money already spent, so the aim is to finish
each window close to empty.

So Medulla steers delegation toward seats that still have headroom. Workers on a
seat with room sort first, and the fullest seat drains first. A seat with too
little headroom to be worth a task drops to cooldown and is skipped by automatic
assignment. Each seat's remaining headroom is written into what the orchestrator
reads, so it can size a task's token budget to a seat that actually fits it. All
usage is metered back to the seat it was drawn from.

Tokenmax is a preference and never a block, so an explicitly targeted worker
still runs even on an exhausted seat. The accounting fails open too: if seat
information is unavailable, delegation proceeds normally rather than stalling.
Budget accounting is soft by contract, so a gap in Medulla's own bookkeeping
costs you some over-delegation rather than a halted operation.

Published allowances are estimates, since providers do not publish exact
per-window numbers. Medulla treats its own ceilings as a starting guess and
corrects them against what providers actually do, so a seat that gets throttled
earlier than predicted is backed off regardless of what the estimate said.

Seats can be prioritized, parked without disconnecting, or pinned to specific
workers. Cost figures attached to a plan are reporting only, showing how much
value you extracted from something you already bought. They are never billed.

## Seeing where it went

Settings has a Usage page, also reachable with `/usage`, showing this session's
spend broken out by tier, a sub-agent row with its task count, and per-task
detail underneath. When you are logged in it adds account totals: plan, spend and
call count for the cycle, remaining balance, and a per-model breakdown.

The Sessions tab carries the live view, with context used against the window per
row and a bar for the selected agent. Watching that fill is usually how you
notice a task is shaped wrong.
