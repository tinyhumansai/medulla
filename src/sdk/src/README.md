# Src

The Rust module tree for the `medulla` SDK crate. `lib.rs` defines the public surface; child folders separate transport, runtime, orchestration, integration, persistence, and UI-facing responsibilities.

## Contents

- [`agents/`](./agents/) — Where agent templates come from: the built-in coding catalog, the on-disk `.medulla/agents/*.toml` store that supersedes it, and the installer that writes the catalog into that store so it can be edited.
- [`auth/`](./auth/) — Login plumbing: an RFC 8252 loopback OAuth flow against the Medulla backend and the pure URL/query helpers the CLI and tests share.
- [`bridge/`](./bridge/) — Message delivery bridges for local and remote agent communication.
- [`client/`](./client/) — HTTP/SSE client for the Medulla orchestration backend.
- [`clipboard/`](./clipboard/) — Clipboard writers: try a platform binary (pbcopy / clip / wl-copy / xclip / xsel) then fall back to OSC 52 (hand the text to the terminal). OSC 52 is the only mechanism that survives SSH, so it backstops rather than replaces the spawn path.
- [`config/`](./config/) — `medulla.tui.json`-compatible config — the subset the TUI reads, plus a `backend` section for the HTTP runtime. Permissive: missing fields take defaults, unknown fields are ignored.
- [`contacts/`](./contacts/) — Incoming contact-request management for tiny.place peers.
- [`core_host/`](./core_host/) — Boot the embedded OpenHuman core in this process.
- [`daemon/`](./daemon/) — The headless `medulla daemon`: offer this machine's local coding-agent CLIs (Claude Code / Codex / OpenCode) as an addressable tiny.place agent over Signal end-to-end encrypted DMs, speaking both plain-text prompts and the `medulla-task/1` task protocol an orchestrator delegates with.
- [`flow_engine/`](./flow_engine/) — The adapter seam between Medulla and the `tinyflows` workflow engine.
- [`harness_contract/`](./harness_contract/) — Public agent-harness wire-contract types.
- [`harness_work/`](./harness_work/) — What a coding-agent harness is *working on*, in one vocabulary.
- [`history_upload/`](./history_upload/) — Sharing local coding-agent history to earn onboarding credit.
- [`hub/`](./hub/) — The task-sender hub — the outbound half of the harness plane.
- [`init/`](./init/) — Workspace initialisation: registering a directory and authoring its `MEDULLA.md`.
- [`logging/`](./logging/) — The one line-sink type every subsystem narrates through.
- [`onboarding/`](./onboarding/) — First-run worker registration orchestration.
- [`runtime/`](./runtime/) — The `Runtime` trait the UI drives, plus its snapshot contract. Concrete implementations live alongside: `openhuman` (the embedded core, which the product runs on) and `mock` (tests and demos). The UI depends only on the trait and its types.
- [`session_history/`](./session_history/) — Recent-session history for local harness sessions.
- [`sessions/`](./sessions/) — Interactive coding-agent session management: the two lifetime classes, the two turn-source drivers, and the machinery that runs them.
- [`protocol/`](./protocol/) — Medulla's own wire protocol for the medulla TUI/daemon.
- [`ui/`](./ui/) — UI-facing data surface shared with the terminal app: `events` (the folded event log + `TuiEvent`), `agents` lane folding, `stream` token/thread derivations, the `chat_store`, the `work` panel over a harness's own todos and sub-agents, and small `util` helpers. Rendering-heavy screens (app, login, composer, theme) and the interactive onboarding screen live in the `medulla-tui` crate, which re-exports these data modules.
- [`update/`](./update/) — Release update checking and self-update.
- [`worker_profile/`](./worker_profile/) — The persisted first-run worker profile.
- [`workflows/`](./workflows/) — Authored, durable, multi-step work: workflow definitions and their runs.
- [`wrapper/`](./wrapper/) — The transparent harness wrapper behind `medulla codex` / `medulla claude` / `medulla opencode`.
- [`clock_tests.rs`](./clock_tests.rs) — Tests for the clock module.
- [`clock.rs`](./clock.rs) — Wall-clock helpers shared across the crate.
- [`home_tests.rs`](./home_tests.rs) — Tests for the home module.
- [`home.rs`](./home.rs) — The Medulla home directory and the early `.env` loader.
- [`lib.rs`](./lib.rs) — medulla: client SDK for Medulla. The UI-facing surface is driven through a `Runtime` trait; concrete runtimes (backend HTTP/SSE, core socket, mock) live in `runtime`. The HTTP/SSE client lives in `client`. The terminal app that consumes this crate is the sibling `medulla-tui` crate.
- [`persistence.rs`](./persistence.rs) — Shared atomic file persistence.
- [`tokio_tuning.rs`](./tokio_tuning.rs) — Tokio runtime tuning for any process that may host an agent turn.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
