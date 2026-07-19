# RLM: Context Scaling Without Collapse

The naive fix for scale is a bigger context window. It does not work, because the failure is accuracy under load, not capacity. Medulla takes a different route: applying [RLM (Recursive Language Model)](https://arxiv.org/abs/2512.24601) techniques, it manages workloads reaching 10 million tokens while keeping its own reasoning surface small and precise. The bulk of the fleet's traffic never competes with the model's attention.

RLM is a published inference paradigm from MIT CSAIL ([Zhang, Kraska & Khattab, 2025](https://arxiv.org/abs/2512.24601); see also [Alex Zhang's write-up](https://alexzhang13.github.io/blog/2025/rlm/)): rather than reading a long input as one mega-prompt, the model treats it as an external environment it can examine, decompose, and recurse over. Medulla applies that idea to a live fleet instead of a static document.

This is not a theoretical claim. On our heaviest fan-out fixture, where the combined corpus alone exceeds a single model's window, Medulla completed all 50 workers with perfect accuracy for $0.27. A flat single-context baseline failed with an API error on the same task: one turn of tool results was larger than the model window. The architecture that reads everything could not even start.

## Why It Matters for Cost

Because Medulla keeps its reasoning surface small and offloads the bulk, you pay orchestrator rates only on the distilled slice that actually reaches the model, not on the millions of tokens flowing through your fleet. Underneath, Medulla's native workers average around 6,000 tokens per task, where an equivalent full harness session runs about 16x that. Efficiency at the worker level is what makes 1,000-harness fleets economically sane.

Curious how the offload works in the code? See [Architecture › RLM](developers/architecture.md#rlm-keeping-the-reasoning-surface-small).
