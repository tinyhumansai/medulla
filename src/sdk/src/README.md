# Src

The Rust module tree for the `medulla` SDK crate. `lib.rs` defines the public surface; child folders separate transport, runtime, orchestration, integration, persistence, and UI-facing responsibilities.

## Contents

- [`agent/`](./agent/) — Medulla's own local agent: a tinyagents harness, a tool surface, and one turn driver.
- [`attribution/`](./attribution/) — Git commit attribution for Medulla-launched harnesses: the flags and environment that carry it, resolved from config.
- [`auth/`](./auth/) — Login plumbing: an RFC 8252 loopback OAuth flow against the Medulla backend and the pure URL/query helpers the CLI and tests share.
- [`bridge/`](./bridge/) — Message delivery bridges for local and remote agent communication.
- [`client/`](./client/) — HTTP/SSE client for the Medulla orchestration backend.
- [`clipboard/`](./clipboard/) — Clipboard writers: try a platform binary (pbcopy / clip / wl-copy / xclip / xsel) then fall back to OSC 52 (hand the text to the terminal). OSC 52 is the only mechanism that survives SSH, so it backstops rather than replaces the spawn path.
- [`codex_app_server/`](./codex_app_server/) — A pooled client for `codex app-server`: one long-lived Codex process serving many concurrent threads, so a fan-out of lanes costs one runtime rather than one each.
- [`codex_overrides/`](./codex_overrides/) — Codex `-c` config overrides that make a routed Codex run actually reach a non-OpenAI model: the provider block, the API-key auth preference, and a model catalog derived at spawn time from the one the installed Codex cached for itself.
- [`config/`](./config/) — `medulla.tui.json`-compatible config — the subset the TUI reads, plus a `backend` section for the HTTP runtime. Permissive: missing fields take defaults, unknown fields are ignored.
- [`control_socket/`](./control_socket/) — The control plane a spawned harness reaches Medulla's live fleet through: the orchestrator binds a unix socket, and the MCP shim proxies its fleet tools over it.
- [`daemon/`](./daemon/) — The headless `medulla daemon`: offer this machine's local coding-agent CLIs (Claude Code / Codex / OpenCode) as an addressable tiny.place agent over Signal end-to-end encrypted DMs, speaking both plain-text prompts and the `medulla-task/1` task protocol an orchestrator delegates with.
- [`flow_engine/`](./flow_engine/) — The adapter seam between Medulla and the `tinyflows` workflow engine.
- [`harness_contract/`](./harness_contract/) — Public agent-harness wire-contract types.
- [`harness_hooks/`](./harness_hooks/) — Standardized lifecycle hooks for Medulla-launched harnesses: declare a hook once and Medulla installs it into whichever coding CLI it spawns.
- [`harness_transcript/`](./harness_transcript/) — What a harness actually said while it served one task, kept for replay: the ordered account of each message, tool call, and error, for a workflow `agent` node's headless run.
- [`harness_work/`](./harness_work/) — What a coding-agent harness is *working on*, in one vocabulary.
- [`history_upload/`](./history_upload/) — Sharing local coding-agent history to earn onboarding credit.
- [`home/`](./home/) — The Medulla home directory and the early `.env` loader.
- [`hub/`](./hub/) — The task-sender hub — the outbound half of the harness plane.
- [`inference_proxy/`](./inference_proxy/) — A loopback HTTP proxy that owns Medulla's OpenRouter attribution, so traffic Medulla orchestrated is credited to Medulla rather than to the harness that composed the request.
- [`init/`](./init/) — Workspace initialisation: registering a directory and authoring its `MEDULLA.md`.
- [`logging/`](./logging/) — The one line-sink type every subsystem narrates through.
- [`mcp/`](./mcp/) — Medulla's Model Context Protocol server, offered to the harnesses it spawns: `workflow_*` authoring/run tools and the `fleet_*` verbs.
- [`onboarding/`](./onboarding/) — First-run worker registration orchestration.
- [`protocol/`](./protocol/) — Medulla's own wire protocol for the medulla TUI/daemon.
- [`runtime/`](./runtime/) — The `Runtime` trait the UI drives, plus its snapshot contract. Concrete implementations live alongside: `cloud` (the orchestration backend, which the product runs on) and `mock` (tests and demos). The UI depends only on the trait and its types.
- [`session_history/`](./session_history/) — Recent-session history for local harness sessions.
- [`sessions/`](./sessions/) — Interactive coding-agent session management: the two lifetime classes, the two turn-source drivers, and the machinery that runs them.
- [`ui/`](./ui/) — UI-facing data surface shared with the terminal app: `events` (the folded event log + `TuiEvent`), `agents` lane folding, `stream` token/thread derivations, the `chat_store`, the `work` panel over a harness's own todos and sub-agents, and small `util` helpers. Rendering-heavy screens (app, login, composer, theme) and the interactive onboarding screen live in the `medulla-tui` crate, which re-exports these data modules.
- [`update/`](./update/) — Release update checking and self-update.
- [`worker_profile/`](./worker_profile/) — The persisted first-run worker profile.
- [`workflows/`](./workflows/) — Authored, durable, multi-step work: workflow definitions and their runs.
- [`wrapper/`](./wrapper/) — The transparent harness wrapper behind `medulla codex` / `medulla claude` / `medulla opencode`.
- [`clock_tests.rs`](./clock_tests.rs) — Tests for the clock module.
- [`clock.rs`](./clock.rs) — Wall-clock helpers shared across the crate.
- [`harness_tools_tests.rs`](./harness_tools_tests.rs) — Tests for the tool-withholding marker.
- [`harness_tools.rs`](./harness_tools.rs) — Whether a harness Medulla launches receives Medulla's own MCP tools.
- [`lib.rs`](./lib.rs) — medulla: client SDK for Medulla. The UI-facing surface is driven through a `Runtime` trait; concrete runtimes (backend HTTP/SSE, mock) live in `runtime`. The HTTP/SSE client lives in `client`. The terminal app that consumes this crate is the sibling `medulla-tui` crate.
- [`persistence.rs`](./persistence.rs) — Shared atomic file persistence.
- [`tokio_tuning.rs`](./tokio_tuning.rs) — Tokio runtime tuning for any process that may host an agent turn.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
