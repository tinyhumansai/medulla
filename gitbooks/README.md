# Medulla v1: The First Orchestrator Model

Medulla v1 is officially announced today. It is the first model of its kind: not a chat model, not another agent harness, but an **orchestrator model**, purpose-built to command fleets of agent harnesses like Claude Code, Codex, and their peers. Medulla v1 brings three capabilities together for the first time:

1. **A 10-million-token effective context**, handled efficiently through RLM (Recursive Language Model) techniques, so accuracy holds at a scale where single-context models collapse.
2. **Live streaming input from every running harness**, so fleet awareness is continuous rather than post-hoc.
3. **Concurrency of up to 1,000 agent harnesses running at the same time**, governed end to end so every operation completes with an answer.

Medulla is currently the only model to bring all three together.

## Correctness First, by Design

Medulla is built around one principle: get the right answer. When a worker fails, it re-delegates. When results look thin, it verifies. When a task splits, it fans out rather than guessing. That discipline is why accuracy holds where other systems collapse, and it is the reason teams trust Medulla with operations too large to eyeball: every task accounted for, every budget enforced, every operation ending in an answer.

## Where to Go Next

- [Why an Orchestrator Model](why-an-orchestrator-model.md) — the failure mode of chat-first orchestration, and what an orchestrator model does differently.
- [RLM: Context Scaling Without Collapse](rlm-context-scaling.md) — how Medulla handles 10 million tokens without losing accuracy.
- [Benchmarks](benchmarks.md) — head-to-head results with full tables.
- [Real-World Fleets](real-world-fleets.md) — the runs behind the numbers.
- [Open Benchmarks, Open SDKs](open-benchmarks-open-sdks.md) — reproduce every number yourself.
- [Pricing and Availability](pricing-and-availability.md) — pricing, early alpha, and how to request access.

## What Comes Next

Models are updated at such a pace that it is easy to forget the harder problem was never any single model's intelligence. It is coordination: making a hundred capable harnesses behave like one coherent operation. Medulla v1 is our first step toward orchestration as a first-class capability, and the numbers in these pages are not projections. They are runs you can reproduce.

Fleets with everyone.
