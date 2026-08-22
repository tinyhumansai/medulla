# Context Scaling Without Collapse

The naive fix for scale is a bigger context window. It does not work, because the limit is accuracy under load rather than capacity. A model's answers degrade well before its window is full, so an orchestrator that reads every harness's raw output into one transcript loses the thread long before it runs out of room. [Why an Orchestrator](why-an-orchestrator-model.md) makes that argument in full; this page is about the machinery that answers it.

## The shape: treat the fleet as an environment

Medulla's route around the problem is to keep the bulk of the fleet's traffic out of the model's attention entirely. Harness output is folded and distilled on the way in, and what the orchestrator reasons over is a short, current view of the operation rather than a record of everything that has happened.

That is the shape of [RLM (Recursive Language Model)](https://arxiv.org/abs/2512.24601), a published inference paradigm from MIT CSAIL ([Zhang, Kraska & Khattab, 2025](https://arxiv.org/abs/2512.24601); see also [Alex Zhang's write-up](https://alexzhang13.github.io/blog/2025/rlm/)). Rather than reading a long input as one mega-prompt, the model treats it as an external environment it can examine, decompose, and recurse over. Medulla applies that to a live fleet instead of a static document.

The environment here is concrete. Output that would otherwise land in the transcript (files the orchestrator read, task results, agent output) goes into a context store as named chunks, shared across the cycle. The orchestrator pages through it deliberately with `context_list`, `context_search`, `context_peek`, and `context_summarize`. Because other managers working the same cycle share the environment, a chunk written by one is readable by another, which is also why chunks are read with care rather than assumed to be yours.

## The two records that carry a long operation

Distillation on the way in solves one turn. Carrying an operation across hours of work needs something that does not grow with the traffic, and two records do that.

The **task ledger** holds one digest per settled delegated task. It is the orchestrator's record of a fan-out: id, instruction, assigned agent, status, timings, event count, budget consumption. Two hundred tasks leave two hundred digests, each a few lines long, and the ledger keeps answering after the agent that ran a task has disconnected.

**Compressed history** holds the running summaries. Each reasoning pass leaves a summary rather than a transcript, roughly 20:1, so a long operation carries its own past forward at a fraction of the size. The compress tier does that work, which is why it is a tier of its own and routed to a model sized for it rather than to the orchestrator's model.

Both are covered from the cost side in [Token Efficiency and Budgets](features/token-efficiency.md).

## A walk-through: what reaches the orchestrator

Take one instruction that fans out to twenty coding tasks across a mixed fleet.

**What does not reach the orchestrator:**

* Raw harness traffic. Worker events are filtered and compressed, or dropped, before they reach a thinking layer at all, which is to say before they cost anything.
* Tool payloads. A tool result past a size limit moves to the context store, and the transcript keeps a pointer and a head excerpt in its place.
* A manager's working. Each cycle deploys 0..N concurrent managers, and a manager's thinking and intermediate tool calls do not ride upward. Only the message from its last turn does.
* The reasoning tier's scratch tools. The orchestrator tier does not see them.
* A chatty harness's event rate. Worker events are debounced, so a harness emitting constantly cannot churn the orchestrator's prompt cache.
* Anything evicted by the context guard. Once utilization crosses a high-water mark, older material moves to the store instead of being left to crowd the window.

**What does reach it:**

* One ledger digest per settled task, with its status and what it cost.
* The distilled result of each delegation, at the size the compress tier left it.
* The running summary of the operation so far.
* Whatever the orchestrator asks the environment for by name, through `context_search` and `context_peek`, when a decision actually needs the detail.
* Questions raised by agents, and the fleet's current state, streamed as they happen.

Nothing here is lost. The full material stays in the context store and the event record, addressable by the orchestrator when a decision turns on it, and readable by you in the terminal app. What changes is that reading it is a deliberate act rather than the default.

## What it buys

Accuracy holds as the fleet grows, because the reasoning surface does not grow with it. The tenth harness and the hundredth cost the orchestrator roughly the same attention, since both arrive as digests rather than transcripts.

It also changes the bill: the orchestrator is metered on the distilled slice, not on the fleet's traffic. [Token Efficiency and Budgets](features/token-efficiency.md) has the numbers and the enforcement mechanisms, and [Pricing and Availability](pricing-and-availability.md) has the rates.

Curious how the offload works in the code? See [Architecture](developers/architecture.md).
