# Codex Server

Codex over a shared `codex app-server` process — the third harness transport, alongside the per-task CLI fork and ACP.

Where those spawn a process per task, this opens a thread on a pooled one, so a fan-out of lanes costs one Codex runtime rather than one each. That is the whole of why it exists, and it is why workflows — which fan out by construction — are what it was built for.

Selected by naming the `codex-server` harness, which reaches here as `HarnessTransport::AppServer` on the task frame, or by `MEDULLA_HARNESS_TRANSPORT=app-server` for callers with no frame to state it on.

## Contents

- [`execution.rs`](./execution.rs) — Thread setup and the turn loop: transport selection, the child environment, `thread/start` / `thread/resume`, and the abort / idle / connection-death paths.
- [`fold.rs`](./fold.rs) — Folding app-server notifications into Medulla's semantic event stream.
- [`mod.rs`](./mod.rs) — Codex over a shared `codex app-server` process.
- [`tests.rs`](./tests.rs) — Unit tests for transport selection and the notification fold.

## What this reports

Deliberately minimal: lifecycle status, the assistant's messages, token usage, and stable worktree reports that determine where a resumed turn executes. The app-server reports far more — per-item reasoning deltas, command output streams, patch previews — and the CLI transport's mappers turn the equivalent into the rich agent-rail detail an operator watches.

Reproducing that surface here would mean a second implementation of every mapper, tracking a wire format still marked experimental, for a transport chosen when throughput is what matters. So a `codex-server` lane reports that it is working, what it finally said, and what it cost — and an operator who wants to watch a lane work runs it on `codex`.

## Maintenance

Keep this index synchronized when responsibilities move. The pooled client itself is [`crate::codex_app_server`](../../../codex_app_server); end-to-end coverage lives in `src/sdk/tests/e2e_codex_app_server.rs`.
