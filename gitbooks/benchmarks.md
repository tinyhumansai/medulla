# Benchmarks

We validated Medulla head to head against a leading open-source agent harness (the same category as [Claude Code](https://www.anthropic.com/claude-code) and [Codex](https://github.com/openai/codex)), run both as a flat baseline and explicitly prompted to orchestrate its own subagents. Same tasks, same underlying models, strict offline scoring against ground truth.

## Heavy Fan-Out, 50 Bulky Sources

The combined corpus alone exceeds a single model's window.

| Engine                     | Accuracy | Cost   | Wall time | Outcome                                       |
| -------------------------- | -------- | ------ | --------- | --------------------------------------------- |
| **Medulla**                | **1.00** | $0.27  | ~4 min    | All 50 workers completed, zero fabrication    |
| Open-source harness (flat) | DNF      | n/a    | 15s       | API error: one turn of results exceeds window |
| Open-source harness (CLI)  | 0.00     | $0.004 | 25s       | Never fanned out, returned empty              |

## Adversarial and Multi-Turn Fixtures

Scores are accuracy / relevancy / groundedness.

| Fixture                                                        | Medulla                | Baseline CLI        |
| -------------------------------------------------------------- | ---------------------- | ------------------- |
| Noise stress (decoys, corrupted segments, injection attempts)  | **1.00 / 1.00 / 1.00** | 0.00 (empty output) |
| Multi-turn steering (mid-run corrections, stale-data redirect) | **1.00 / 1.00 / 1.00** | 0.91 / 0.92         |
| Heterogeneous fan-out (4 task kinds + planted distractors)     | **1.00 / 1.00 / 1.00** | 1.00                |
| Dependency chains (case to locker to registrar)                | **1.00** at $0.074     | 1.00                |

## 100 [Project Euler](https://projecteuler.net/) Problems, Solved in Parallel

| Engine                    | Correct    | Cost  | Wall time                  |
| ------------------------- | ---------- | ----- | -------------------------- |
| **Medulla**               | **83/100** | $0.24 | 5 min                      |
| Open-source harness (CLI) | 0/100      | n/a   | 4.7 min, nothing scoreable |

## Repeatability

Repeatability matters as much as peak scores. Across our repeat matrix, Medulla passed every completed repeat, while the CLI baselines swung between perfect runs, crashes, and 25-minute hangs on identical fixtures. Single-run numbers flatter a flaky system; Medulla's numbers are representative.

Every fixture and the harness that runs them are open source; see [Open Benchmarks, Open SDKs](open-benchmarks-open-sdks.md) to reproduce these numbers.
