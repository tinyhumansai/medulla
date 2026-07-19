# Real-World Fleets

Benchmark scores support the story; the runs themselves tell it better.

**Commanding 100 harnesses at once.** In our widest run, a single delegation spread 100 concurrent workers across a bulky source corpus. Every worker completed. Every tracking code in the final report was real. The parallelism was genuine: summed task time exceeded wall-clock time by 1,635x. One model, one operation, one hundred workers, zero fabrication. And this is not limited to Medulla's own workers: the same orchestration layer drives real harness sessions, dispatching tasks to full CLI instances, re-delegating the sessions that fail, and joining their results into one coherent report.

**Holding the line under hostile input.** We salted a fixture with decoy claims, corrupted segments, stale mirrors, and prompt-injection notices, then asked for a clean report. Medulla returned a perfect score with zero contamination, down to counting the mirror disagreements exactly. The baseline returned empty output on the same input.

**Taking correction mid-flight.** In a multi-turn steering scenario, the operator changed the plan while the fleet was running: skip the decommissioned sources, that figure is battery not serial, add source 13. Medulla honored every correction, and when a mid-archive bulletin contradicted a stale figure, it reported the authoritative 61% over the outdated 18%. It also recovers on its own: when workers fail, Medulla notices and re-delegates them, and when a task truly cannot be recovered, it reports the failure honestly rather than papering over it.

**A hundred problems before your coffee cools.** Given 100 [Project Euler](https://projecteuler.net/) problems to solve in parallel, Medulla returned 83 correct answers in 5 minutes for $0.24. The baseline produced nothing scoreable in roughly the same time.

Underneath all of this is worker efficiency: Medulla's native workers average around 6,000 tokens per task, where an equivalent full harness session runs about 16x that. Efficiency at the worker level is what makes 1,000-harness fleets economically sane.
