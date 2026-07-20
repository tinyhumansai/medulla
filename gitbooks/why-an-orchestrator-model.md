# Why an Orchestrator Model

Agent harnesses like [Claude Code](https://www.anthropic.com/claude-code) and [Codex](https://github.com/openai/codex) are remarkable at running one task deeply. But ask a harness to coordinate other harnesses and you hit the same quiet failure mode everywhere: the orchestrator is just another LLM with a transcript, and every harness it manages writes into that transcript. Model accuracy degrades well before the context window fills. So an orchestrator that reads raw harness traffic stops scaling at a handful of them. Long before the window runs out, it stops being able to think.

Orchestration is becoming the dominant pattern in agentic systems, yet it has been running on architectures designed for chat. A chat model manages one thread. An orchestrator model must hold an entire operation in its head: hundreds of harnesses in flight, work being decomposed and delegated, results streaming back, decisions made continuously under pressure.

Medulla was designed for exactly this. Where a harness drowns in its own coordination noise, Medulla always sees a small, current, high-signal picture of everything happening beneath it, no matter how large the operation grows.

## Built for the Fleet

Medulla does not wait for harnesses to finish and report back. It streams input from every running harness as it happens, including progress, results, and questions, and it can talk back to any of them mid-task. When workers fail, it notices and re-delegates them. When a task truly cannot be recovered, it reports the failure honestly rather than papering over it. Fleet awareness is continuous, not post-hoc.

And concurrency is a governed resource, not an emergent behavior. No task is ever silently dropped, budgets and deadlines are enforced across the entire fleet, and every operation always completes with an answer.
