# Architecture

Medulla is an **orchestrator model** — see
[Why an Orchestrator Model](../why-an-orchestrator-model.md) for the product
argument. This page is about the code: how the open-source SDK and TUI are put
together, how they talk to the backend, and how the pieces named in the product
story map onto modules you can read.

## Two crates

The public repository is a two-crate [Cargo](https://doc.rust-lang.org/cargo/)
workspace with a strict separation between logic and rendering:

* [`src/sdk/`](https://github.com/tinyhumansai/medulla/tree/main/src/sdk) — the
  `medulla` SDK crate: a **UI-free logic library**. The backend HTTP/SSE client,
  the runtime adapters, persona memory, and the tiny.place integration. Reusable
  from any Rust program.
* [`src/tui/`](https://github.com/tinyhumansai/medulla/tree/main/src/tui) — the
  `medulla-tui` crate, shipping the `medulla` binary: a [ratatui](https://ratatui.rs/)
  terminal UI over the SDK. It owns state, rendering, input, and theming, and
  re-exports the SDK's UI-facing data modules.

The rule of thumb: reusable APIs live in the SDK; rendering and process wiring
live in the app crate. The SDK depends only on its own traits and types — never on
the TUI.

## The `Runtime` trait

The UI drives everything through one trait, `Runtime`, plus its snapshot
contract. The UI depends only on that trait — not on any concrete backend — which
is what makes the three runtimes interchangeable and the whole thing testable
offline. Three implementations live alongside it:

* **`backend`** — the [HTTP/SSE](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events)
  client, for the production orchestrator.
* **`core`** — a locally running core-js orchestration server over a
  [NDJSON](https://ndjson.org/) Unix socket (via `core_client`).
* **`mock`** — a scripted runtime for tests and demos, with no network.

At startup the TUI selects one and falls back down the chain
(core → backend → mock) if a preferred runtime is unavailable. See
[Configuration › Runtimes](configuration.md#runtimes).

## The backend client

The [`client`](https://github.com/tinyhumansai/medulla/tree/main/src/sdk/src/client)
module is the HTTP/SSE client for the orchestration backend. Its surfaces:

* **Auth** (`/auth`) — the login and token-verification endpoints behind
  [Authentication](authentication.md).
* **Durable sessions** (`/medulla/v1`) — persistent orchestration sessions.
* **SSE event streaming** — the live event feed the UI folds into agent lanes and
  traces.
* **One-shot orchestration** (`/orchestration/v1`) — fire-and-collect delegation.

Every response is wrapped in a `{ "success": true, "data": ... }` envelope;
errors arrive as `{ "success": false, "error": ..., "errorCode": ... }` and are
surfaced as a typed `ClientError::Api` that preserves the `errorCode`.

## RLM: keeping the reasoning surface small

The reason an orchestrator can command up to 1,000 harnesses without drowning in
their transcripts is that it does **not** read raw fleet traffic into one context
window. Medulla applies **RLM (Recursive Language Model)** techniques — treating
the workload as an external environment the model examines, decomposes, and
recurses over, rather than a single mega-prompt — so it manages workloads reaching
10 million tokens while keeping its own reasoning surface small and precise.

RLM is a published inference paradigm from MIT CSAIL (Zhang, Kraska & Khattab,
2025); see the [paper](https://arxiv.org/abs/2512.24601) and
[Alex Zhang's write-up](https://alexzhang13.github.io/blog/2025/rlm/) for the
technique, and [RLM: Context Scaling Without Collapse](../rlm-context-scaling.md)
for what it buys Medulla in accuracy and cost. The RLM machinery itself is
server-side; the SDK's job is to stream the distilled, high-signal slice to and
from the UI.

## The UI data surface

The SDK's [`ui`](https://github.com/tinyhumansai/medulla/tree/main/src/sdk/src/ui)
module is the UI-facing data surface shared with the terminal app: the folded
event log and `TuiEvent`, agent-lane folding, token/thread stream derivations, the
chat store, and small helpers. Rendering-heavy screens (app, login, composer,
theme) live in the `medulla-tui` crate, which re-exports these data modules — so
the data model and the rendering stay on opposite sides of the crate boundary.

## Persona memory

The [`memory`](https://github.com/tinyhumansai/medulla/tree/main/src/sdk/src/memory)
module is a thin, medulla-owned wrapper over a persona-memory layer. It turns
local coding-agent history into a durable, prompt-ready persona pack and exposes a
small offline query surface (`status` / `search` / `directives` / `overview`)
plus an LLM-backed ingest path. Vendor types never cross the module boundary:
every result is translated into a serde-friendly, medulla-owned type so the UI and
protocol layers stay decoupled.

## tiny.place integration

Medulla's orchestration layer does not only drive its own native workers — it can
dispatch tasks to full coding-agent CLI instances (Claude Code, Codex, OpenCode)
running anywhere, over [tiny.place](https://tiny.place), the agent-to-agent
network. Two modules make that work:

* [`tinyplace`](https://github.com/tinyhumansai/medulla/tree/main/src/sdk/src/tinyplace) —
  the protocol and agent-runtime layer. Session envelopes and harness event types
  are re-exported from the published `tinyplace` Rust SDK so medulla and the SDK
  share one wire model. On top of that sit the `medulla-tinyplace/1` **task frame
  protocol** (delegated work over encrypted DMs), owner→machine **control frames**
  (session-targeted input), a receiver-side **consumer** that folds the harness
  stream into a live session view, a derived session-**status** state machine, and
  agent-runtime helpers (a file-backed session store, identity bootstrap, and
  async mailbox / contact / presence loops).
* [`daemon`](https://github.com/tinyhumansai/medulla/tree/main/src/sdk/src/daemon) —
  the headless [`medulla daemon`](cli-reference.md#medulla-daemon), which offers a
  machine's local coding-agent CLIs as an addressable tiny.place agent over
  [Signal](https://signal.org/docs/) end-to-end encrypted DMs. It speaks both
  plain-text prompts and the `medulla-tinyplace/1` task protocol. Internally it is
  split into transcript **mappers** (JSONL → semantic events), **providers**
  (detection + one-shot headless execution), a **capabilities** probe, an
  encrypted-DM **transport**, and the provider-agnostic **task loop** state
  machine.

The same layer powers the [harness wrappers](cli-reference.md#harness-wrappers)
(`medulla codex` / `claude` / `opencode`), which bridge an interactive local
session to an owner over tiny.place. tiny.place itself uses
[Signal-protocol](https://signal.org/docs/) encryption for messaging and
[x402](https://www.x402.org/) for agent payments.

## Testing philosophy

Because the UI depends only on the `Runtime` trait and the client speaks a small
set of HTTP/SSE surfaces, the whole system can be exercised offline. The test
suites spin up in-process stand-ins — a mock HTTP/SSE backend, a mock core
Unix-socket server, a mock tiny.place API server, and mock `claude`/`codex`/`opencode`
CLIs that emit realistic provider stream-JSONL. Tests stay deterministic and need
no live network. See [Contributing](contributing.md) for how to run them.

## Where to read next

* [Configuration](configuration.md) — how the runtime is selected and configured.
* [CLI Reference](cli-reference.md) — the daemon and wrappers in operational
  detail.
* [Open Benchmarks, Open SDKs](../open-benchmarks-open-sdks.md) — the model is
  gated; this code is not.
