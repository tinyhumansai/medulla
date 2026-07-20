---
description: >-
  Medulla remembers two different things: who you are as an operator, and what it
  learned mid-operation. They are separate systems with separate lifetimes.
---

# Memory

Most agent tooling starts every session from nothing. You re-explain your stack,
your conventions, and your standing rules, and the moment the session ends that
context evaporates. Medulla keeps two kinds of memory so it doesn't have to ask
twice — and they are deliberately different things.

| | Persona memory | Orchestrator memory |
| --- | --- | --- |
| **Remembers** | Who you are and how you work | Facts discovered during an operation |
| **Built from** | Your past coding-agent sessions | The reasoning tier, as it goes |
| **Lifetime** | Durable, across everything | Durable across cycles on a host |
| **Written** | By an explicit ingest you run | By the model, when it decides to |

## Persona memory

Persona memory turns the coding-agent history already sitting on your machine —
Claude Code transcripts, Codex rollouts, the instruction files and git history
under your project roots — into a compact, prompt-ready **persona pack**.

The point is that you have already explained yourself, hundreds of times, to
harnesses that forgot. Persona memory reads that record and distils the stable
parts: how you like code written, what your stack is, which rules you state over
and over. What comes out is short enough to put in front of a model on every run.

### What it distils

* **Directives** — explicit standing rules, folded in verbatim from your
  instruction files. These are the things you have written down because you meant
  them.
* **Observations** — prescriptive statements inferred from your history, each
  tagged with a **facet** (`coding_style`, `stack`, and peers) and a confidence
  **tier** from `t0` to `t3`, usually with the supporting quote it came from.
* **The pack** — a compiled `PERSONA.md`, the prompt-ready artifact everything
  else exists to produce.

Confidence tiers matter: a rule you wrote down explicitly and a pattern inferred
from three transcripts should not carry the same weight, and they don't.

### Running it

Ingest never happens behind your back. It reads your history and calls a model,
so you start it deliberately:

```sh
medulla memory status              # what's in the pack right now
medulla memory backfill            # walk everything, oldest first
medulla memory ingest              # incremental: only what changed since last time
medulla memory compile             # rebuild the pack from what's stored (offline)
medulla memory search "<query>"    # search the corpus (offline)
```

Backfill is the first run and can take minutes. After that, `ingest` moves the
cursor forward over what's new. `compile`, `search`, and `status` never call a
model and never cost anything.

The same controls live on the TUI's **Memory** tab: `b` to backfill, `i` to
ingest new, `r` to refresh, and the pack, facet counts, and directives rendered
alongside. A run in flight says so on its own line, because a backfill that takes
minutes with no visible sign reads like a hang. Starting a second run while one
is going is refused rather than queued — ingest costs money.

In chat, `/memory` loads the pack and `/memory <query>` searches it.

### Cost, and the ceiling on it

Ingest is the one part of Medulla that spends money on your behalf without a task
attached, so it carries an explicit ceiling. `maxCostUsd` caps provider spend per
run (default $5), and a run that hits the cap stops early and reports that it
did, rather than quietly truncating or quietly continuing.

Inference resolves the same way `medulla init` does: an explicit
`OPENROUTER_API_KEY` wins, otherwise Medulla uses the backend's inference surface
with the token from `medulla login`. With neither, memory still runs — `status`,
`search`, and `compile` are fully local — and ingest tells you plainly what it
needs.

### Where it lives, and what it doesn't do

The workspace defaults to `<medulla-home>/memory` (typically `~/.medulla/memory`),
with the pack at `persona/PERSONA.md` inside it. It is local, on-disk, and
independent of which runtime is driving your chat — persona memory works the same
whether you are on the backend, a local core socket, or the mock.

It is enabled by default and configured under the `memory` section (or the
matching environment variables) — see
[Configuration](../developers/configuration.md) for the full key list and
[CLI Reference › `medulla memory`](../developers/cli-reference.md#medulla-memory)
for every flag.

Persona memory is read-only to the orchestrator. It can search the corpus, pull
directives, and check status; it cannot write to your persona. What Medulla
believes about you changes only when you run an ingest.

## Orchestrator memory

The second system is much smaller and does a different job. During an operation
the reasoning tier can persist a fact under a key and read it back on a later
cycle — a durable scratch space that survives the passes of a long run.

This matters because a large operation is not one model call. It is many, and
without somewhere to put a hard-won conclusion, the same conclusion gets derived
again. The scratch space is the model's own notebook: it decides what is worth
keeping, and nothing is captured automatically.

It is scoped to the reasoning layer by design — the orchestrator itself never
sees these tools. Keeping the orchestrator's surface clear of scratch state is
the same discipline that keeps it clear of raw fleet traffic; see
[RLM: Context Scaling Without Collapse](../rlm-context-scaling.md).

Related but distinct: the **task ledger** (one digest per settled delegated task,
the orchestrator's record of a fan-out) and **compressed history** (the running
summaries that carry a long operation forward). Those are covered in
[Token Efficiency and Budgets](token-efficiency.md).
