![Hero Image](./docs/screen.png)

# Medulla v1: The First Orchestrator Model

Medulla v1 is the first model of its kind: not a chat model, not another agent harness, but an **orchestrator model**, purpose-built to command fleets of agent harnesses like [Claude Code](https://www.anthropic.com/claude-code), [Codex](https://github.com/openai/codex), and their peers. Medulla v1 brings three capabilities together for the first time:

1. **A 10-million-token effective context**, handled efficiently through [RLM (Recursive Language Model)](https://arxiv.org/abs/2512.24601) techniques, so accuracy holds at a scale where single-context models collapse.
2. **Live streaming input from every running harness**, so fleet awareness is continuous rather than post-hoc.
3. **Concurrency of up to 1,000 agent harnesses running at the same time**, governed end to end so every operation completes with an answer.

Medulla is currently the only model to bring all three together.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
```

This downloads the prebuilt `medulla` binary for your platform, verifies its SHA-256 against the release manifest (when a checksum tool such as `sha256sum`, `shasum`, or `openssl` is available), and installs to `~/.medulla/bin`. If the installer updated your `PATH`, reload your shell first, with `exec $SHELL` or a new terminal, so `medulla` resolves. Then:

```sh
medulla login   # browser OAuth; stores a verified JWT
medulla         # bare invocation starts the TUI
```

With no credentials, `medulla` opens a login screen. Press `m` there to explore offline against the mock runtime. See [For developers](#for-developers) to build from source or embed the SDK.

Full documentation: **[tinyhumans.gitbook.io/medulla](https://tinyhumans.gitbook.io/medulla)**

## What It Does

Five capabilities do most of the work. Each has a full page in the [documentation](https://tinyhumans.gitbook.io/medulla).

**[Memory](https://tinyhumans.gitbook.io/medulla/features/memory).** Medulla reads the coding-agent history already on your machine, meaning Claude Code transcripts, Codex rollouts, and your instruction files, then distils it into a compact persona pack covering your standing rules, your stack, and how you like code written. You have already explained yourself hundreds of times to harnesses that forgot. Separately, the reasoning tier keeps a durable scratch space so a hard-won fact survives to the next cycle instead of being derived twice.

**[Workers and sessions](https://tinyhumans.gitbook.io/medulla/features/workers-and-sessions).** A worker is capacity, meaning a real harness running with your credentials in your workspace. A session is the thread you return to, resumable and forkable, surviving the terminal app that started it. Unassigned tasks go to the least-loaded healthy worker, failed ones get re-delegated, and every task settles into a definite state.

**[MEDULLA.md](https://tinyhumans.gitbook.io/medulla/features/workspace-profiles).** A short authored file at a repository root telling the orchestrator what the directory is and how to route work over it. `AGENTS.md` is written for an agent working inside a repo, so it is too long and silent on routing. This is roughly 100 to 200 tokens the orchestrator reads every cycle, and `medulla init` drafts one from what your repo already has.

**[Routing](https://tinyhumans.gitbook.io/medulla/features/routing).** Deciding how to decompose a problem, executing a step, and compressing a transcript are different jobs. Medulla splits them across three cognitive tiers, orchestrator, reasoning, and compress, and routes each to a model sized for it. Workspace profiles and per-task hints steer harness and model choice as advisory guidance rather than hard gates.

**[Token efficiency and budgets](https://tinyhumans.gitbook.io/medulla/features/token-efficiency).** Two opposite problems. On spending less, bulk fleet output never reaches the orchestrator, so its reasoning surface stays small and you pay orchestrator rates on the distilled slice only. On wasting less, if you have connected paid subscriptions those tokens are already bought, so Medulla steers delegation toward seats with headroom, because leaving them unused at the end of a window is money thrown away.

Validated head-to-head against a leading open-source agent harness with strict offline scoring against ground truth. Full tables, methodology, token pricing, and the runs behind the numbers are in the [documentation](https://tinyhumans.gitbook.io/medulla), and every fixture and the harness that runs them are open source, so you can reproduce them.

## Availability

Medulla v1 is in **early alpha**, and access is exclusive and gated. It is rolling out to a small group of OpenHuman subscribers first, alongside gated API access for select teams building serious agentic systems. Alpha partners get direct access to the team, and their workloads shape what Medulla becomes.

Request access and tell us what you are orchestrating.

## Documentation

The full documentation is at **[tinyhumans.gitbook.io/medulla](https://tinyhumans.gitbook.io/medulla)**.

**Overview**

- [Why an Orchestrator Model](https://tinyhumans.gitbook.io/medulla/why-an-orchestrator-model)
- [RLM: Context Scaling Without Collapse](https://tinyhumans.gitbook.io/medulla/rlm-context-scaling)
- [Benchmarks](https://tinyhumans.gitbook.io/medulla/benchmarks)
- [Real-World Fleets](https://tinyhumans.gitbook.io/medulla/real-world-fleets)
- [Open Benchmarks, Open SDKs](https://tinyhumans.gitbook.io/medulla/open-benchmarks-open-sdks)
- [Pricing and Availability](https://tinyhumans.gitbook.io/medulla/pricing-and-availability)

**Features**, what Medulla does day to day:

- [Memory](https://tinyhumans.gitbook.io/medulla/features/memory): the persona pack, and the orchestrator's scratch space.
- [Workers and Sessions](https://tinyhumans.gitbook.io/medulla/features/workers-and-sessions): capacity, threads, and what survives.
- [MEDULLA.md Workspace Profiles](https://tinyhumans.gitbook.io/medulla/features/workspace-profiles): telling the orchestrator what a repo is.
- [Orchestrator Routing](https://tinyhumans.gitbook.io/medulla/features/routing): cognitive tiers, harness selection, runtime fallback.
- [Token Efficiency and Budgets](https://tinyhumans.gitbook.io/medulla/features/token-efficiency): small surfaces, enforced budgets, tokenmax.

**Developers**, to install the TUI, embed the SDK, and wire your own fleet to the orchestrator:

- [Getting Started](https://tinyhumans.gitbook.io/medulla/developers/getting-started): install, build, run, first login.
- [CLI Reference](https://tinyhumans.gitbook.io/medulla/developers/cli-reference): the TUI, the daemon, the harness wrappers, self-update.
- [Configuration](https://tinyhumans.gitbook.io/medulla/developers/configuration): the Medulla home, layered config, and the three runtimes.
- [Authentication](https://tinyhumans.gitbook.io/medulla/developers/authentication): the browser loopback login flow and token handling.
- [Architecture](https://tinyhumans.gitbook.io/medulla/developers/architecture): the SDK/TUI split, runtime adapters, RLM, and the tiny.place bridge.
- [Contributing](https://tinyhumans.gitbook.io/medulla/developers/contributing): build, test, lint, coverage, and releasing.

## For developers

This repo hosts the open-source Medulla Rust workspace: the [`medulla`](https://github.com/tinyhumansai/medulla/tree/main/src/sdk) SDK library and the [`medulla-tui`](https://github.com/tinyhumansai/medulla/tree/main/src/tui) app crate that ships the `medulla` binary, a [ratatui](https://ratatui.rs/) terminal UI over the orchestrator.

The prebuilt binary installs with the one-liner under [Install](#install) above. To build from source instead:

```sh
cargo install --path src/tui   # installs the `medulla` binary
medulla                        # bare invocation starts the TUI
```

Full developer documentation, covering CLI subcommands, configuration, authentication, architecture, and how to build from source, lives in the [Developers](https://tinyhumans.gitbook.io/medulla/developers) section of the docs.

## Why an Orchestrator Model

Agent harnesses like Claude Code and Codex are remarkable at running one task deeply. But ask a harness to coordinate other harnesses and you hit the same quiet failure mode everywhere: the orchestrator is just another LLM with a transcript, and every harness it manages writes into that transcript. Model accuracy degrades well before the context window fills. So an orchestrator that reads raw harness traffic stops scaling at a handful of them. Long before the window runs out, it stops being able to think.

Orchestration is becoming the dominant pattern in agentic systems, yet it has been running on architectures designed for chat. A chat model manages one thread. An orchestrator model must hold an entire operation in its head: hundreds of harnesses in flight, work being decomposed and delegated, results streaming back, decisions made continuously under pressure. Medulla was designed for exactly this. Where a harness drowns in its own coordination noise, Medulla always sees a small, current, high-signal picture of everything happening beneath it, no matter how large the operation grows.

Fleets with everyone.
