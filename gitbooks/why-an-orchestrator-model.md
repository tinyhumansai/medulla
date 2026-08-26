# Why an Orchestrator

Agent harnesses like [Claude Code](https://www.anthropic.com/claude-code) and [Codex](https://github.com/openai/codex) are very good at running one task deeply. Orchestration is a different job, and there are two usual ways of reaching for it. Split the terminal — tmux, a pane each — and drive them yourself. Or point a harness at other harnesses: one more chat session, driving the rest.

Both work for three or four. Both stop working past that, and neither stops for reasons that have anything to do with how capable the model is.

## The terminal multiplexer was never the answer

tmux solves a real problem, and it is not this one. It gives you panes; it has no idea what is in them. Which agent is blocked on a permission prompt, which one died twenty minutes ago, which one is finished and waiting for you to look — a multiplexer cannot tell you, because to it they are all just bytes on a pseudo-terminal. You are the scheduler. You cycle the panes, you read them, you notice. That is a job that scales linearly with the number of agents and holds for about as long as your attention does.

Medulla runs the same processes, on the same kind of PTY, and reads them. Every session it opens has its own terminal emulator kept live in the background whether or not you are looking at it, and Medulla watches all of them for the things that actually need you: a permission prompt, a startup dialog, a blocking error, a terminal bell, a session that died, a turn that finished and is waiting on review. Those roll up to one mark per row and one count in the rail's title — *`⚠ 3 waiting on you`* — so a fleet you are not watching still tells you when it needs you.

That is the difference in one line. A multiplexer gives you N things to check. Medulla tells you which of the N to look at, and puts you there in a keystroke.

## The shape mismatch

A chat model manages one thread. It reads a transcript, it answers, the transcript grows. An orchestrator has to hold an entire operation: harnesses in flight, work being decomposed and delegated, results arriving out of order, decisions taken continuously while all of that is still moving.

Run that operation as a chat and every harness underneath writes into the same transcript. In our own use, model accuracy falls off well before a context window is full, so an orchestrator reading raw harness traffic degrades long before it runs out of room. Adding a bigger window does not fix it, because the constraint is attention rather than capacity. [Context Scaling Without Collapse](rlm-context-scaling.md) covers what Medulla does instead.

The rest of this page is about the failure modes that follow from the shape, and what an orchestrator has to do differently.

## What breaks

### Fleet awareness arrives late

A chat-shaped orchestrator learns about a harness when that harness finishes and reports back. Anything that happens in between (a plan that went sideways twenty minutes ago, a question the harness is blocked on, a result that already invalidates two other tasks) is invisible until the turn ends.

Medulla streams input from every running harness as it happens, covering progress, results, and questions, and it can talk back to any of them mid-task. That is also what makes steering possible: you can correct a plan or answer an agent's question while the fleet is running, and the operation absorbs the change rather than restarting.

### Failure gets papered over

When a delegated task fails, a chat orchestrator has one natural move, which is to summarize the failure into the transcript and carry on. Nothing forces the task to reach a definite state, and nothing distinguishes "this failed" from "I stopped mentioning it".

Medulla records every delegated task in a ledger: its id, instruction, assigned agent, status, timings, event count, and budget consumption. Each task settles as done, failed, or cancelled. A failed worker is re-delegated, cancelling one task aborts exactly that task and leaves its siblings running, and a task that genuinely cannot be recovered is reported as failed. The ledger stays authoritative even when the agent that ran the task has disconnected, so it answers what happened rather than what was last said about it.

### Concurrency is whatever the model felt like

Fan-out in a chat orchestrator is an emergent property of prompting. Nothing bounds how many harnesses run at once, how deep delegation recurses, how long the whole thing may take, or what it may spend.

Medulla makes those explicit caps: a per-cycle token draw, deadline, and concurrency cap; per-task step and token allowances; a depth cap on recursion; and a daily account limit. The concurrency cap is a semaphore rather than a scheduler, so excess tasks queue and run as slots free up instead of being rejected for arriving at a busy moment. A two-hundred task fan-out completes under a cap of eight. See [Token Efficiency and Budgets](features/token-efficiency.md).

### Running out of budget ends the operation

In a chat orchestrator, a hard limit surfaces as an exception, and an exception in the middle of a fan-out loses the operation. Medulla reports exhaustion in-band: the model receives an error it can reason about and recover from, and a cycle always produces a reply. Termination is guaranteed by construction, through pass ceilings, a forced final turn, the budget gate, and the depth cap.

### Every kind of thinking costs the same

Deciding how to decompose a problem, carrying out a step, and squeezing a verbose transcript into something short are three different jobs with different price tags. A single chat session pays top-tier rates for all three. Medulla splits them into cognitive tiers (orchestrator, reasoning, compress) and routes each to a model sized for it. See [Orchestrator Routing](features/routing.md).

### Placement is a guess

Handed five repositories and a fleet of harnesses, a model with no other information will guess where work belongs, and its guesses will read plausibly whether or not they are right. Medulla narrows the guess from two directions. Capability probing asks an agent what it can actually reach (working directory, accessible directories, git project and branch, tools, MCP servers, provider backends) and caches the answer. [`MEDULLA.md`](features/workspace-profiles.md) profiles let an operator state what a repository is and how work over it should be routed, in roughly 100 to 200 tokens per workspace.

## What an orchestrator does instead

Medulla separates deciding from doing. The orchestrator tier holds the operation: it decides what happens next, reads the distilled picture of the fleet, funds and reviews delegations. It does not fan out itself. Each cycle it deploys 0..N concurrent managers, fixing each manager's host and workspace for that manager's whole run, and each manager then chooses the harness, targets or spawns agents, and delegates tasks to them.

That split is what keeps the top of the stack narrow. The orchestrator tier is deliberately the smallest surface in the system: it never reads raw fleet traffic, and it does not even see the reasoning tier's scratch tools. Delegation is also detached, so a delegation returns immediately and the operation continues rather than blocking until the fan-out drains.

## Neither, then

The two workarounds fail from opposite ends. The multiplexer knows nothing about the work, so you supply all the judgement. The chat-shaped orchestrator supplies judgement but drowns in the traffic, because every agent it manages writes into the one transcript it thinks with.

Medulla is the third option: a process built to hold an operation — a registry of live sessions with no fixed ceiling, a ledger that makes every task settle, explicit caps on fan-out and spend, and a reasoning surface deliberately kept small enough to stay accurate while the fleet grows. You open another agent with `Ctrl-T`. Nothing about the twentieth is different from the second.

## Read next

* [Context Scaling Without Collapse](rlm-context-scaling.md) for how the reasoning surface is kept small.
* [Orchestrator Routing](features/routing.md) for the tiers and how work reaches a harness.
* [Workers and Sessions](features/workers-and-sessions.md) for what a worker is and how tasks are assigned.
* [Architecture](developers/architecture.md) for how this maps onto modules you can read.
