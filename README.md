# Medulla v1: The First Orchestrator Model

Medulla v1 is the first model of its kind: not a chat model, not another agent harness, but an **orchestrator model**, purpose-built to command fleets of agent harnesses like Claude Code, Codex, and their peers. Medulla v1 brings three capabilities together for the first time:

1. **A 10-million-token effective context**, handled efficiently through RLM (Recursive Language Model) techniques, so accuracy holds at a scale where single-context models collapse.
2. **Live streaming input from every running harness**, so fleet awareness is continuous rather than post-hoc.
3. **Concurrency of up to 1,000 agent harnesses running at the same time**, governed end to end so every operation completes with an answer.

Medulla is currently the only model to bring all three together.

## Why an Orchestrator Model

Agent harnesses like Claude Code and Codex are remarkable at running one task deeply. But ask a harness to coordinate other harnesses and you hit the same quiet failure mode everywhere: the orchestrator is just another LLM with a transcript, and every harness it manages writes into that transcript. Model accuracy degrades well before the context window fills. So an orchestrator that reads raw harness traffic stops scaling at a handful of them. Long before the window runs out, it stops being able to think.

Orchestration is becoming the dominant pattern in agentic systems, yet it has been running on architectures designed for chat. A chat model manages one thread. An orchestrator model must hold an entire operation in its head: hundreds of harnesses in flight, work being decomposed and delegated, results streaming back, decisions made continuously under pressure. Medulla was designed for exactly this. Where a harness drowns in its own coordination noise, Medulla always sees a small, current, high-signal picture of everything happening beneath it, no matter how large the operation grows.

## Benchmarks at a Glance

Validated head to head against a leading open-source agent harness (the same category as Claude Code and Codex), with strict offline scoring against ground truth:

| Benchmark                                | Medulla                | Baseline harness             |
| ---------------------------------------- | ---------------------- | ---------------------------- |
| Heavy fan-out, 50 bulky sources          | **1.00** at $0.27      | DNF (window exceeded) / 0.00 |
| Noise stress (decoys, injection, decay)  | **1.00 / 1.00 / 1.00** | 0.00 (empty output)          |
| Multi-turn steering                      | **1.00 / 1.00 / 1.00** | 0.91 / 0.92                  |
| Dependency chains                        | **1.00** at $0.074     | 1.00                         |
| 100 Project Euler problems in parallel   | **83/100** in 5 min    | 0/100                        |

Full tables, methodology, and the runs behind them are in the [documentation](gitbooks/README.md). Every fixture and the harness that runs them are open source, so you can reproduce every number.

## Pricing

|                     | Price           |
| ------------------- | --------------- |
| Input tokens        | $3 / million    |
| Cached input tokens | $0.10 / million |
| Output tokens       | $6 / million    |

Because Medulla keeps its reasoning surface small and offloads the bulk, you pay orchestrator rates only on the distilled slice that actually reaches it, not on the millions of tokens flowing through your fleet.

## Availability

Medulla v1 is in **early alpha**, and access is exclusive and gated. It is rolling out to a small group of OpenHuman subscribers first, alongside gated API access for select teams building serious agentic systems. Alpha partners get direct access to the team, and their workloads shape what Medulla becomes.

Request access and tell us what you are orchestrating.

## Documentation

The full documentation lives in [gitbooks/](gitbooks/README.md):

- [Why an Orchestrator Model](gitbooks/why-an-orchestrator-model.md)
- [RLM: Context Scaling Without Collapse](gitbooks/rlm-context-scaling.md)
- [Benchmarks](gitbooks/benchmarks.md)
- [Real-World Fleets](gitbooks/real-world-fleets.md)
- [Open Benchmarks, Open SDKs](gitbooks/open-benchmarks-open-sdks.md)
- [Pricing and Availability](gitbooks/pricing-and-availability.md)

Fleets with everyone.

## Rust SDK & TUI

This repo also hosts the `medulla` Rust crate — the client SDK and terminal UI in one package:

- `src/client/` — HTTP/SSE client for the Medulla backend API (auth, durable sessions, streaming events, the orchestration tool loop).
- `src/` — a ratatui terminal UI over that API: chat with the orchestrator, watch agent lanes, traces, and context live, plus the tinyplace integration that brings agent channels together (`src/tinyplace_support/`, `src/daemon/`).

Build and run:

```sh
cargo install --path .        # installs the `medulla` binary
medulla login                 # browser login; stores credentials
medulla                       # bare invocation starts the TUI
```

`medulla login` runs a browser-based OAuth loopback flow (`--provider google|github|twitter|discord`, `--no-browser` to copy the URL, or `--token <64-hex>` for headless one-time tokens) and saves a verified JWT to `<config-dir>/medulla/credentials.json`; the TUI picks it up automatically. The loopback callback is guarded by a random state nonce so a page sharing the loopback origin can't forge it. `medulla logout` clears it. You can also pass a token directly with `MEDULLA_TOKEN=<jwt> medulla`. If you start `medulla` with no working credentials, the TUI shows an in-terminal login screen (browser flow, paste-a-token, or `m` to continue against the scripted mock runtime). Other subcommands:

- `medulla daemon` — headless coding-agent daemon serving claude/codex/opencode over encrypted tiny.place DMs.
- `medulla codex` / `medulla claude` / `medulla opencode` — launch the real coding-agent CLI in your terminal exactly as if run directly (inherited stdio; unrecognized flags pass through verbatim), while bridging the session to tiny.place underneath: the wrapper tails the harness transcript, forwards it as encrypted `SessionEnvelopeV2` DMs to the configured owner, and injects owner control-frame input into the child. `--no-bridge` runs a plain passthrough. Configured by `TINYPLACE_HARNESS_DM_TO` / `TINYPLACE_OPENHUMAN_OWNER` (and the `TINYPLACE_<P>_*` overrides); see `DEVELOPING.md`.
- `medulla sessions` — list recent claude/codex sessions as JSON.

The backend base URL defaults to production (`https://api.tinyhumans.ai`), and tiny.place to `https://api.tiny.place`. Set `MEDULLA_STAGING=1` to switch both to staging (`https://staging-api.tinyhumans.ai` / `https://staging-api.tiny.place`). Precedence for the backend URL is `MEDULLA_API_URL` > config-file `backend.baseUrl` > staging/prod default; see `DEVELOPING.md`.
