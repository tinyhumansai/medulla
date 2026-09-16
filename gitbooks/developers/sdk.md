---
description: >-
  The medulla Rust SDK: the crate layout, its cargo features, the Backend trait
  and its implementations, the backend client, and where to read next in source.
---

# The Rust SDK

`medulla` is the library crate at [`src/sdk/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/): a UI-free logic
library holding the account-side backend client and the `Backend` trait over
it, the in-process agent loop, the coding-agent daemon, sessions, the local
dispatch hub, workflows, the host-link integration, and the UI-facing data
surface the terminal app renders.
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

## The `Backend` trait

The backend's part in Medulla is small and account-shaped, and it is all behind
one trait, [`medulla::backend::Backend`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/backend/mod.rs).
Nothing in it touches sessions, hosts, or dispatch; those are local. The UI
depends only on the trait, which is what makes the implementations
interchangeable and the whole app runnable offline.

| Method | Purpose |
| --- | --- |
| `describe()` | A human-readable line naming what backs this instance. Required rather than defaulted, so an implementation cannot accidentally report itself as a scripted demo. |
| `team_usage()` | Account-level usage for Settings › Usage. `Ok(None)` means unsupported. |
| `logout()` | Forget this host's stored session. Defaults to reporting that there is nothing to log out of, which is the honest answer for a backend holding no credential. |
| `list_feedback(query)`, `feedback_detail(id)`, `vote_feedback(…)`, `comment_feedback(…)`, `submit_feedback(…)` | The public feedback board behind Settings › Feedback. Each defaults to `Ok(None)` or a "no board here" error. |

Plan entitlement is deliberately not a method. [`medulla::access`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/access/)
reads the facts `/auth/me` reports (the plan, the registration instant, the
server's clock) and `decide` turns them into a verdict in the binary; the
backend is asked whether this account may run Medulla and nothing more.

Three implementations ship, all under
[`src/sdk/src/backend/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/backend/):

* `backend::CloudBackend` is what a signed-in session runs on. It drives the
  account API through a [`MedullaClient`](#the-backend-client).
  [`backend::connect`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/backend/connect/)
  builds that client from config and the environment (`client_from_config`),
  and `readiness` / `probe` report whether it is usable as `Readiness::Ready`,
  `Readiness::SignedOut`, or `Readiness::Unusable(reason)`, before anything
  paints.
* `backend::MockBackend` is the scripted offline stand-in behind `--mock` and
  the TUI's feature suites, with a feedback board that retallies votes and keeps
  comments for the life of the process.
* `backend::OfflineBackend` is what a signed-out run holds: every call answers
  that there is no backend, and the rest of the app carries on.

## The backend client

[`medulla::client::MedullaClient`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/client/) is a typed surface
over the shared `tinyhumans-sdk` transport, not a second HTTP client. The SDK
owns credential headers, the `{success, data}` envelope, path percent-encoding,
and the not-exposed-route gate; this module adds typed DTOs for the handful of
routes Medulla calls (`/auth/me`, account usage, history reward, the feedback
board) and the `ClientError` taxonomy that front ends branch on through one
predicate (`is_auth_error`).

```rust
use medulla::client::MedullaClient;

let client = MedullaClient::new("https://api.tinyhumans.ai", jwt);
// or, to share one reqwest::Client with the rest of the process:
let client = MedullaClient::builder()
    .base_url("https://api.tinyhumans.ai")
    .jwt(jwt)
    .http_client(http)
    .build();
```

`DEFAULT_BASE_URL` is `http://localhost:5000`. Submodules: `error/` (the error
type and the conversion from `tinyhumans_sdk::Error`), `types/` (JSON types
mirroring backend responses), and `feedback/` (the feedback-board calls).

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

`mock_link_forwarder.rs` is a blind loopback UDP relay for the
[relayed route](host-link-protocol.md#appendix-a-relayed-route-test-harness-only)
of the host-link protocol, which exists only so the coordination end-to-end
harness can put a relay between two endpoints in-process. `coordination_owner`
is the client end of that harness: it provisions a pair, dispatches task frames
over the link, and prints the terminal frame as JSON. See
[Testing](testing.md#the-coordination-end-to-end-harness).

## The module tree

Every module carries its own `README.md` and `//!` docs; those are the API source
of truth. `lib.rs` defines the public surface.

| Module | Responsibility |
| --- | --- |
| [`access/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/access/) | Who may use Medulla: the plan-entitlement verdict derived from `/auth/me`. |
| [`agent/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/agent/) | Medulla's own local agent: a `tinyagents` harness, a tool surface (`fs`, `shell`, and the guard around them), and one turn driver. What the `openhuman` harness id runs on. |
| [`attribution/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/attribution/) | Git commit attribution: the `Co-authored-by` trailer and the hook shims that carry it without disabling a repository's own hooks. |
| [`auth/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/auth/) | An RFC 8252 loopback OAuth flow against the backend, plus the pure URL and query helpers the CLI and tests share. |
| [`backend/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/backend/) | The `Backend` trait and its `CloudBackend`, `MockBackend`, and `OfflineBackend` implementations, plus `connect`. |
| [`bridge/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/bridge/) | Message delivery bridges for local and remote agent communication. |
| [`client/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/client/) | The typed HTTP client for the account-side backend routes. |
| [`clipboard/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/clipboard/) | Clipboard writers: a platform binary first, then OSC 52. See [Troubleshooting](troubleshooting.md#copying-out-of-medulla). |
| [`codex_app_server/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/codex_app_server/) | A pooled client for `codex app-server`. See [Harness integration](harness-integration.md#codex-on-a-shared-process). |
| [`codex_overrides/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/codex_overrides/) | Codex `-c` config overrides that make a routed Codex run reach a non-OpenAI model. |
| [`config/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/config/) | The `medulla.tui.json`-compatible config the TUI reads, plus the `backend` section. Permissive: missing fields take defaults, unknown fields are ignored. |
| [`control_socket/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/control_socket/) | The local control socket a spawned harness reaches, and the grant tokens that scope it. |
| [`daemon/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/daemon/) | The headless `medulla daemon`: offering this machine's coding-agent CLIs as an addressable agent, over plain prompts and the `medulla-task/1` protocol. |
| [`fleet/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/fleet/) | The declared-capacity contracts: the `Host → Harness → Workspace → Agent` chain, agent declarations, and the `CapacitySnapshot` roll-up. |
| [`flow_engine/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/flow_engine/) | The adapter seam between Medulla and the `tinyflows` workflow engine (`workflows` feature). |
| [`harness_contract/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/harness_contract/) | The public agent-harness wire-contract types. See [Harness integration](harness-integration.md#the-wire-contract). |
| [`harness_hooks/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/harness_hooks/) | The hooks Medulla installs into a launched harness, and the launch policy around them. |
| [`harness_transcript/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/harness_transcript/) | What a harness actually said while it served one task, kept for replay. |
| [`harness_work/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/harness_work/) | What a coding-agent harness is working on, in one vocabulary. |
| [`history_upload/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/history_upload/) | Sharing local coding-agent history to earn onboarding credit. |
| [`home/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/home/) | The Medulla home directory and the early `.env` loader. |
| [`hub/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/hub/) | The device-local dispatch hub: the worker roster, the `TaskRunner`, the activity log, and the `fleet_*` seam workflows and MCP tools dispatch through. |
| [`inference_proxy/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/inference_proxy/) | The loopback attribution proxy. See [Attribution and routing](attribution-and-routing.md). |
| [`logging/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/logging/) | The one line-sink type every subsystem narrates through. |
| [`mcp/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/mcp/) | Medulla's own MCP server, offered to the harnesses it spawns (`workflows` feature). |
| [`onboarding/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/onboarding/) | First-run worker registration for the standing daemon. |
| [`protocol/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/protocol/) | Medulla's own wire protocol for the TUI and daemon, plus the centralized environment-variable resolution both share. |
| [`session_history/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/session_history/) | Recent-session history for local harness sessions. |
| [`subscriptions/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/subscriptions/) | Subscription-usage meters: how much of each paid allowance (Claude, Codex, OpenRouter, TinyHumans) has been spent. |
| [`sessions/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/sessions/) | Interactive coding-agent session management: the two lifetime classes, the two turn-source drivers, and the machinery that runs them. |
| [`ui/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/ui/) | The UI-facing data surface: `events`, `agents` lane folding, `stream` derivations, `chat_store`, the `work` panel, and `util`. Rendering lives in `medulla-tui`. |
| [`update/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/update/) | Release update checking and self-update. |
| [`worker_profile/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/worker_profile/) | The persisted first-run worker profile. |
| [`workflows/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/workflows/) | Authored, durable, multi-step work: workflow definitions and their runs (`workflows` feature). |
| [`wrapper/`](https://github.com/tinyhumansai/medulla-src/tree/main/src/sdk/src/wrapper/) | The transparent harness wrapper behind `medulla codex`, `medulla claude`, and `medulla opencode`. |

Four files sit at the top level beside them: `clock.rs` (wall-clock helpers),
`harness_tools.rs` (whether a harness Medulla launches receives Medulla's own
MCP tools), `persistence.rs` (shared atomic file persistence, crate-private),
and `tokio_tuning.rs` (Tokio runtime tuning for any process that may host an
agent turn).

## Read next

* [Architecture](architecture.md): how the SDK and the TUI fit together.
* [Testing](testing.md): the suites and stand-ins that exercise this crate.
* [Environment variables](environment-variables.md): what the crate reads at runtime.
