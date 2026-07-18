# RLM: Context Scaling Without Collapse

The naive fix for scale is a bigger context window. It does not work, because the failure is accuracy under load, not capacity. Medulla takes a different route: applying RLM (Recursive Language Model) techniques, it manages workloads reaching 10 million tokens while keeping its own reasoning surface small and precise. The bulk of the fleet's traffic never competes with the model's attention.

This is not a theoretical claim. On our heaviest fan-out fixture, where the combined corpus alone exceeds a single model's window, Medulla completed all 50 workers with perfect accuracy for $0.27. A flat single-context baseline failed with an API error on the same task: one turn of tool results was larger than the model window. The architecture that reads everything could not even start.

## Why It Matters for Cost

Because Medulla keeps its reasoning surface small and offloads the bulk, you pay orchestrator rates only on the distilled slice that actually reaches the model, not on the millions of tokens flowing through your fleet. Underneath, Medulla's native workers average around 6,000 tokens per task, where an equivalent full harness session runs about 16x that. Efficiency at the worker level is what makes 1,000-harness fleets economically sane.
