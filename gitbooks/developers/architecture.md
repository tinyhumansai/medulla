# Architecture

See [Why an Orchestrator](../why-an-orchestrator-model.md) for the product argument. This page is about the code: how the open-source SDK and TUI are put together, how they talk to the backend, and how the pieces named in the product story map onto modules you can read.

## Two crates

The public repository is a two-crate [Cargo](https://doc.rust-lang.org/cargo/) workspace with a strict separation between logic and rendering:

* [`src/sdk/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/) is the `medulla` SDK crate, a UI-free logic library. It holds the backend HTTP/SSE client, the runtime adapters over the embedded core, sessions, workflows, and the host-link integration. It is reusable from any Rust program.
* [`src/tui/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/tui/) is the `medulla-tui` crate, shipping the `medulla` binary: a [ratatui](https://ratatui.rs/) terminal UI over the SDK. It owns state, rendering, input, and theming, and re-exports the SDK's UI-facing data modules.

Reusable APIs live in the SDK; rendering and process wiring live in the app crate. The SDK depends only on its own traits and types, never on the TUI.

## The `Runtime` trait

The UI drives everything through one trait, `Runtime`, plus its snapshot contract. The UI depends only on that trait, not on any concrete implementation, which is what makes the runtimes interchangeable and the whole thing testable offline. Two implementations ship:

* `openhuman`, the embedded OpenHuman core, which is what the product runs on. It boots inside the `medulla` process, so there is no socket and no attach handshake.
* `mock`, a scripted runtime for tests and demos, with no network, reached with `--mock`.

Alongside them sit the pieces both share:

* [`capabilities/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/runtime/capabilities/): narrow capability interfaces over the compatibility-facing `Runtime`.
* [`event_log/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/runtime/event_log/): bounded event storage per conversation thread.
* [`fleet/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/runtime/fleet/): the declared-capacity contracts. This is the strict `Host → Harness → Workspace → Agent` containment chain, the agent-template catalog, and the `CapacitySnapshot` the UI renders.
* [`headless/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/runtime/headless/): the non-interactive driver behind [`medulla run`](cli-reference.md#medulla-run). It attaches a runtime, submits one instruction, and streams folded events as JSON lines.

See [Configuration › Runtimes](configuration.md#runtimes).

## The backend client

The [`client`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/client/) module is the HTTP/SSE client for the orchestration backend. Its surfaces:

* Auth (`/auth`): the login and token-verification endpoints behind [Authentication](authentication.md).
* Durable sessions (`/medulla/v1`): persistent orchestration sessions.
* SSE event streaming: the live event feed the UI folds into agent lanes and traces.
* One-shot orchestration (`/orchestration/v1`): fire-and-collect delegation.

Every response is wrapped in a `{ "success": true, "data": ... }` envelope; errors arrive as `{ "success": false, "error": ..., "errorCode": ... }` and are surfaced as a typed `ClientError::Api` that preserves the `errorCode`.

## Distillation is server-side

The orchestrator does not read raw fleet traffic into one context window. Harness output is folded and distilled before it reaches the orchestrator's reasoning surface, and that machinery runs server-side. The SDK's job is to stream the distilled slice to and from the UI; the folding the client itself does is in [`ui`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/ui/) (see below).

For the argument behind that design and the accuracy and cost figures, see [Token Efficiency](../features/token-efficiency.md) and [Context Scaling Without Collapse](../rlm-context-scaling.md).

## The UI data surface

The SDK's [`ui`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/ui/) module is the UI-facing data surface shared with the terminal app: the folded event log and `TuiEvent`, agent-lane folding, token/thread stream derivations, the chat store, and small helpers. Rendering-heavy screens (app, login, composer, theme) live in the `medulla-tui` crate, which re-exports these data modules, so the data model and the rendering stay on opposite sides of the crate boundary.

The `memory` module, its CLI, and its config schema were removed along with the engine dependency they needed. See [The TUI](the-tui.md#the-tabs).

## Sessions

The [`sessions`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/sessions/) module owns interactive coding-agent session management: the two lifetime classes, the two turn-source drivers, and the machinery that runs them. Sessions are driven from exactly two sources, a `medulla-task/1` task frame or a `tinyplace.harness.session.v*` envelope, and [`input/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/sessions/input/) is the one place that knows the difference.

The rest splits by responsibility:

* [`routing/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/sessions/routing/) decides which lifetime a stimulus gets and whether a provider can serve it interactively at all.
* [`interactive/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/sessions/interactive/) is the transport: one long-lived harness process fed newline-delimited JSON turns over stdin.
* [`completion/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/sessions/completion/) detects when an interactive turn is done.
* [`registry/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/sessions/registry/) tracks which harness session id a conversation is bound to, and serializes per key so two turns cannot interleave onto one transcript.
* [`turn_stream/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/sessions/turn_stream/) is the mode-independent half of running a turn.

## Workflows

[`workflows`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/workflows/) owns authored, durable, multi-step work, both definitions and their runs, and [`flow_engine`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/flow_engine/) is the adapter seam onto the vendored `tinyflows` engine. Engine coupling stays in the seam.

What makes Medulla's use of that engine different from any other host embedding it is that an `agent` node is a *dispatched task*, not a model call. [`run/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/workflows/run/) executes and resumes, [`store/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/workflows/store/) is where workflows and run records live, [`authoring.rs`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/workflows/authoring.rs) edits a graph as a series of checked patches, [`ops/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/workflows/ops/) exposes the whole thing as one JSON-in/JSON-out surface, and [`mcp/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/workflows/mcp/) serves those same operations to a harness over MCP. The whole module sits behind the default `workflows` feature, so a slim build can drop the engine and its jq expression stack.

## Host-link integration

Medulla's orchestration layer drives its own native workers, and it can also dispatch tasks to full coding-agent CLI instances (Claude Code, Codex, OpenCode) running anywhere, over the **host link**. Four modules make that work:

* [`link`](https://github.com/tinyhumansai/medulla-src/tree/main/src/link/) is the transport itself, specified in [`host-link-protocol.md`](../../docs/host-link-protocol.md). Two endpoints exchange UDP datagrams carrying mosh-style state synchronisation through a forwarder, served by the same backend as the rest of the API, which routes bytes it cannot read. The outer header is readable by the forwarder for routing and replay defence; the payload is ChaCha20-Poly1305 under a pair key only the endpoints hold.
* [`bridge`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/bridge/) is the common message-delivery contract. `LocalBridge` routes messages through an isolated in-memory bus when both endpoints run on the same device and no remote service is needed. `LinkBridge` wraps the host link when a peer lives remotely. Task routing depends on this contract rather than on either transport directly, so choosing a delivery scope does not change the task-frame protocol.
* [`protocol`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/protocol/) is the wire model: the `medulla-task/1` task frame protocol (delegated work), owner→machine control frames (session-targeted input), a receiver-side consumer that folds the harness stream into a live session view, and a derived session-status state machine. The `tinyplace.harness.session.v*` envelope version tags are retained verbatim because they are on-the-wire strings; changing one breaks the protocol.
* [`daemon`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/daemon/) is the headless [`medulla daemon`](cli-reference.md#medulla-daemon), which offers a machine's local coding-agent CLIs as an addressable worker over the host link. It speaks both plain-text prompts and the `medulla-task/1` task protocol. Internally it is split into transcript mappers (JSONL to semantic events), providers (detection plus one-shot headless execution), a capabilities probe, a transport, and the provider-agnostic task loop state machine.

The same layer powers the [harness wrappers](cli-reference.md#harness-wrappers) (`medulla codex` / `claude` / `opencode`), which bridge an interactive local session to an owner over the host link.

## Testing philosophy

Because the UI depends only on the `Runtime` trait and the client speaks a small set of HTTP/SSE surfaces, the whole system can be exercised offline. Tests stay deterministic and need no live network.

Unit tests live in a module's sibling `tests.rs`. Cross-module and end-to-end suites live in the owning crate's `tests/` directory, `src/sdk/tests/` and `src/tui/tests/`, named by behavior (`e2e_core.rs`, `feature_workers.rs`). The app crate's tests reach the SDK's stand-ins with `#[path]`.

The shared stand-ins are in [`src/sdk/tests/support/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/tests/support/):

| File | What it stands in for |
| --- | --- |
| `mock_backend.rs` | The orchestration backend's HTTP and SSE surfaces. |
| `fake_app_server.rs` | The app-facing server endpoints. |
| `fake_provider.rs` | A coding-agent provider. |
| `mock_harness.rs`, `mock_harness_helpers.rs`, `mock_harness_script.rs`, `mock_harness_types.rs` | Mock `claude`/`codex`/`opencode` CLIs that emit realistic provider stream-JSONL, plus the scripting and types behind them. |
| `mock_openrouter.rs` | The OpenRouter inference endpoint. |

See [Contributing](contributing.md) for how to run them.

## Read next

* [Configuration](configuration.md): how the runtime is selected and configured.
* [CLI Reference](cli-reference.md): the daemon and wrappers in operational detail.
* [Workflows](../features/workflows.md): the authored multi-step surface built on `workflows` and `flow_engine`.
