# Context Scaling Without Collapse

The naive fix for scale is a bigger context window. It does not work, because the failure is accuracy under load, not capacity. A model's answers degrade well before its window is full, so an orchestrator that reads every harness's raw output into one transcript loses the thread long before it runs out of room.

Medulla takes a different route: the bulk of the fleet's traffic never competes with the model's attention. Harness output is folded and distilled on the way in, so what the orchestrator reasons over is a small, current, high-signal picture rather than a transcript of everything that has happened.

This is the same idea as [RLM (Recursive Language Model)](https://arxiv.org/abs/2512.24601), a published inference paradigm from MIT CSAIL ([Zhang, Kraska & Khattab, 2025](https://arxiv.org/abs/2512.24601); see also [Alex Zhang's write-up](https://alexzhang13.github.io/blog/2025/rlm/)): rather than reading a long input as one mega-prompt, the model treats it as an external environment it can examine, decompose, and recurse over. Medulla applies that shape to a live fleet instead of a static document.

## Why It Matters for Cost

Because the reasoning surface stays small and the bulk is offloaded, you pay orchestrator rates on the distilled slice that actually reaches the model, not on everything flowing through your fleet.

Two related mechanisms carry a long operation without growing the surface. The **task ledger** holds one digest per settled delegated task, which is the orchestrator's record of a fan-out. **Compressed history** holds the running summaries that carry a long operation forward. Both are covered in [Token Efficiency and Budgets](features/token-efficiency.md).

Curious how the offload works in the code? See [Architecture](developers/architecture.md).
