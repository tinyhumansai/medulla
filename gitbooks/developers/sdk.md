---
description: >-
  The medulla Rust SDK: the crate layout, its cargo features, the Runtime trait
  and its implementations, the backend client, and where to read next in source.
---

# The Rust SDK

`medulla` is the library crate at [`src/sdk/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/): a UI-free logic
library holding the backend HTTP and SSE client, the runtime adapters over it,
the in-process agent loop, the coding-agent daemon, sessions, workflows, the
host-link integration, and the UI-facing data surface the terminal app renders.
The `medulla-tui` crate consumes it; nothing in the SDK depends on the TUI.

## Adding it

The repository vendors its path dependencies, so a git dependency needs no extra
setup:

```toml
[dependencies]
medulla = { git = "https://github.com/tinyhumansai/medulla-src", tag = "v0.3.0" }
```

Building from a checkout requires the submodule init described in
[Vendoring](vendoring.md#initialize).

## Cargo features

The crate has one feature.

| Feature | Default | What it turns on |
| --- | --- | --- |
| `workflows` | on | The [`flow_engine`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/flow_engine/) adapter seam onto the vendored `tinyflows` engine, the [`workflows`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/workflows/) module (definitions, store, runs, authoring), and the [`mcp`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/mcp/) module. Kept behind a feature so a slim build can drop the engine and its jq expression stack; on by default because the shipped TUI exposes workflows. |

`mcp` is gated on `workflows` because its `workflow_*` tool family delegates to
`workflows::ops`. The `fleet_*` family beside it depends only on
`control_socket`.

The `cloud` runtime is not behind a feature. It is the runtime the SDK hosts,
and a build without it would have nothing to offer but the offline mock.

## The `Runtime` trait

Everything the UI drives goes through one trait,
[`medulla::runtime::Runtime`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/runtime/mod.rs), plus its
snapshot contract. The UI depends only on the trait and its types, which is what
makes the implementations interchangeable and the whole thing testable offline.

Core methods:

| Method | Purpose |
| --- | --- |
| `describe()` | A human-readable line naming what backs this runtime. Required rather than defaulted, so an implementation cannot accidentally report itself as a scripted demo. |
| `snapshot()` | The current UI-facing state, as a `RuntimeSnapshot`. Synchronous. |
| `subscribe()` | A `broadcast::Receiver<()>` that pings after every event or mutation. Synchronous. |
| `submit(input)` | Submit one user instruction to the active session. |
| `submit_settles_cycle()` | Whether a resolved `submit` means the cycle finished, or only that it was accepted. |
| `submit_with_receipt(input)` | Like `submit`, returning a `SubmitReceipt` when the wire carries a correlation id. Defaults to delegating to `submit` with no receipt. |
| `abort()` | Request cancellation of the active cycle. |
| `logout()` | Forget this host's stored session. Defaults to reporting that there is nothing to log out of, which is the honest answer for a runtime holding no credential. |
| `team_usage()` | Account-level usage, when this runtime has a backend. `Ok(None)` means unsupported. |

Two implementations ship, both under
[`src/sdk/src/runtime/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/runtime/):

* `runtime::openhuman::OpenHumanRuntime` is what the product runs on. It wraps
  the OpenHuman core booted in this process (`OpenHumanRuntime::new(core)`, or
  `with_hub(core, hub)` when the outbound dispatch hub is wired in), so there is
  no socket and no attach handshake.
* `runtime::mock::MockRuntime` is a scripted offline runtime for tests and demos.
  `MockRuntime::demo()` gives a populated snapshot (a roster, presence, a couple
  of turns, a completed delegated task); `MockRuntime::empty()` gives a bare one.
  It also exposes scripting seams: `script_event`, `set_workers`, `set_running`,
  `recorded_calls`, `recorded_handoffs`.

Earlier versions carried an HTTP and SSE cloud-backend runtime and a
`medulla-serve` unix-socket runtime. Both were removed once the core was
embedded; the module docs in `lib.rs` still name them.

Beside the trait sit three supporting modules:

* `runtime::capabilities` narrows the compatibility-facing `Runtime` into
  focused capability interfaces.
* `runtime::fleet` holds the declared-capacity contracts: the
  `Host → Harness → Workspace → Agent` chain, the agent-template catalog, and the
  `CapacitySnapshot` roll-up.
* `runtime::headless` is a non-interactive driver over the trait.

## Driving a runtime headlessly

`runtime::headless::drive_once` attaches a runtime, submits exactly one
instruction, streams the folded events to a writer as NDJSON, and returns once
the cycle result lands. It is generic over `Runtime`, so it works against the
embedded core in production and against the mock in tests.

```rust
use std::sync::Arc;

use medulla::runtime::headless::{drive_once, HeadlessOptions};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut out = std::io::stdout();
    let summary = drive_once(
        runtime,
        "summarize the open tasks".to_string(),
        &mut out,
        HeadlessOptions::default(),
    )
    .await?;
    eprintln!("{} events streamed", summary.events_streamed);
    Ok(())
}
```

The output contract is one JSON object per line, each tagged by a `type` field:
a single `ready` line carrying `describe()` and the session id, one `event` line
per folded event with `seq` and `at`, and a terminal `result` line carrying
`passCount`. Failures come back as a typed `HeadlessError` (`AttachTimeout`,
`Unavailable`, `UnavailableMidCycle`, `SubmitRejected`, `CycleTimeout`,
`Output`) rather than being written into the transcript, so a caller can map each
to an exit code by variant. `HeadlessOptions` bounds the two waits with
`ready_timeout` (30 s by default) and `cycle_timeout` (300 s).

## The backend client

[`medulla::client::MedullaClient`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/client/) is a typed surface
over the shared `tinyhumans-sdk` transport, not a second HTTP client. The SDK
owns credential headers, the `{success, data}` envelope, path percent-encoding,
and the not-exposed-route gate; this module adds typed DTOs for routes the SDK
returns as open JSON, the `ClientError` taxonomy that front ends branch on
through one predicate (`is_auth_error`), and the SSE event stream, which the
SDK's body-buffering transport cannot serve.

```rust
use medulla::client::MedullaClient;

let client = MedullaClient::new("https://api.tinyhumans.ai", jwt);
// or, to share one reqwest::Client across ordinary requests and the SSE stream:
let client = MedullaClient::builder()
    .base_url("https://api.tinyhumans.ai")
    .jwt(jwt)
    .http_client(http)
    .build();
```

`DEFAULT_BASE_URL` is `http://localhost:5000`. Submodules: `error/` (the error
type and the conversion from `tinyhumans_sdk::Error`), `types/` (JSON types
mirroring backend responses), `program/` (models shared by the worker-roster and
task-program endpoints), and `sse/` (a hand-rolled Server-Sent Events parser and
a reconnecting stream).

## Examples

[`src/sdk/examples/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/examples/) holds runnable examples that
demonstrate a narrow contract without becoming production entry points.

`harness_contract_decode.rs` is the executable seam the integration umbrella's
cross-repository contract test drives. It reads one JSON value from stdin, exits
non-zero if serde rejects it, and writes the canonical re-serialized value to
stdout:

```rust
use std::io::{self, Read};

use medulla::harness_contract::TrackedTask;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let task: TrackedTask = serde_json::from_str(&input)?;
    println!("{}", serde_json::to_string(&task)?);
    Ok(())
}
```

`mock_link_forwarder.rs` is a blind loopback UDP forwarder implementing
[section 5](host-link-protocol.md#5-forwarder-rules) of the host-link protocol,
standing in for the backend in the coordination end-to-end harness.
`coordination_owner` is the orchestrator end of that harness: it enrolls a pair,
dispatches task frames over the link, and prints the terminal frame as JSON. See
[Testing](testing.md#the-coordination-end-to-end-harness).

## The module tree

Every module carries its own `README.md` and `//!` docs; those are the API source
of truth. `lib.rs` defines the public surface.

| Module | Responsibility |
| --- | --- |
| [`agents/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/agents/) | Where agent templates come from: the built-in coding catalog, the on-disk `.medulla/agents/*.toml` store that supersedes it, and the installer between them. |
| [`attribution/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/attribution/) | Git commit attribution: the `Co-authored-by` trailer and the hook shims that carry it without disabling a repository's own hooks. |
| [`auth/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/auth/) | An RFC 8252 loopback OAuth flow against the backend, plus the pure URL and query helpers the CLI and tests share. |
| [`bridge/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/bridge/) | Message delivery bridges for local and remote agent communication. |
| [`client/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/client/) | The HTTP and SSE client for the orchestration backend. |
| [`clipboard/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/clipboard/) | Clipboard writers: a platform binary first, then OSC 52. See [Troubleshooting](troubleshooting.md#copying-out-of-medulla). |
| [`codex_app_server/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/codex_app_server/) | A pooled client for `codex app-server`. See [Harness integration](harness-integration.md#codex-on-a-shared-process). |
| [`codex_overrides/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/codex_overrides/) | Codex `-c` config overrides that make a routed Codex run reach a non-OpenAI model. |
| [`config/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/config/) | The `medulla.tui.json`-compatible config the TUI reads, plus the `backend` section. Permissive: missing fields take defaults, unknown fields are ignored. |
| [`control_socket/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/control_socket/) | The local control socket a spawned harness reaches, and the grant tokens that scope it. |
| [`core_host/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/core_host/) | Booting the embedded OpenHuman core in this process. |
| [`daemon/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/daemon/) | The headless `medulla daemon`: offering this machine's coding-agent CLIs as an addressable agent, over plain prompts and the `medulla-task/1` protocol. |
| [`flow_engine/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/flow_engine/) | The adapter seam between Medulla and the `tinyflows` workflow engine (`workflows` feature). |
| [`harness_contract/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/harness_contract/) | The public agent-harness wire-contract types. See [Harness integration](harness-integration.md#the-wire-contract). |
| [`harness_hooks/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/harness_hooks/) | The hooks Medulla installs into a launched harness, and the launch policy around them. |
| [`harness_work/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/harness_work/) | What a coding-agent harness is working on, in one vocabulary. |
| [`history_upload/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/history_upload/) | Sharing local coding-agent history to earn onboarding credit. |
| [`home/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/home/) | The Medulla home directory and the early `.env` loader. |
| [`hub/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/hub/) | The task-sender hub: the outbound half of the harness plane. |
| [`inference_proxy/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/inference_proxy/) | The loopback attribution proxy. See [Attribution and routing](attribution-and-routing.md). |
| [`init/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/init/) | Workspace initialisation: registering a directory and authoring its `MEDULLA.md`. |
| [`logging/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/logging/) | The one line-sink type every subsystem narrates through. |
| [`mcp/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/mcp/) | Medulla's own MCP server, offered to the harnesses it spawns (`workflows` feature). |
| [`onboarding/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/onboarding/) | First-run worker registration orchestration. |
| [`protocol/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/protocol/) | Medulla's own wire protocol for the TUI and daemon, plus the centralized environment-variable resolution both share. |
| [`runtime/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/runtime/) | The `Runtime` trait, its snapshot contract, and the `openhuman` and `mock` implementations. |
| [`session_history/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/session_history/) | Recent-session history for local harness sessions. |
| [`sessions/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/sessions/) | Interactive coding-agent session management: the two lifetime classes, the two turn-source drivers, and the machinery that runs them. |
| [`ui/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/ui/) | The UI-facing data surface: `events`, `agents` lane folding, `stream` derivations, `chat_store`, the `work` panel, and `util`. Rendering lives in `medulla-tui`. |
| [`update/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/update/) | Release update checking and self-update. |
| [`worker_profile/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/worker_profile/) | The persisted first-run worker profile. |
| [`workflows/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/workflows/) | Authored, durable, multi-step work: workflow definitions and their runs (`workflows` feature). |
| [`wrapper/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/wrapper/) | The transparent harness wrapper behind `medulla codex`, `medulla claude`, and `medulla opencode`. |

Three files sit at the top level beside them: `clock.rs` (wall-clock helpers),
`persistence.rs` (shared atomic file persistence, crate-private), and
`tokio_tuning.rs` (Tokio runtime tuning for any process that may host an agent
turn).

## Read next

* [Architecture](architecture.md): how the SDK and the TUI fit together.
* [Testing](testing.md): the suites and stand-ins that exercise this crate.
* [Environment variables](environment-variables.md): what the crate reads at runtime.
