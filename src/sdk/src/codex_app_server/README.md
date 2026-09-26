# Codex App Server

A pooled client for `codex app-server` — one long-lived Codex process serving many concurrent threads.

The CLI transport forks `codex exec` once per task, and each fork pays the whole startup cost and holds its own copy of that state. The app-server holds one runtime and opens a thread per conversation, so ten lanes are ten threads inside one process rather than ten of everything. What it costs is the CLI's per-step rendering; see [`../daemon/providers/codex_server`](../daemon/providers/codex_server) for the minimal fold this transport does provide.

## Contents

- [`connection.rs`](./connection.rs) — One supervised `codex app-server` child process, shared by every thread the pool hands it to: the reader/writer tasks, the pending-request table, per-thread notification fan-out, and the approval auto-responder.
- [`jsonrpc.rs`](./jsonrpc.rs) — The line-framed JSON-RPC dialect `codex app-server` speaks: one JSON object per line, no `Content-Length` framing.
- [`mod.rs`](./mod.rs) — A pooled client for `codex app-server`.
- [`pool.rs`](./pool.rs) — The process-sharing seam: connections keyed by identity, opened once and reused by every task that can safely share them.
- [`tests.rs`](./tests.rs) — Unit tests for the pure parts: the JSON-RPC dialect, the sharing key, and the thread-parameter mapping.
- [`types.rs`](./types.rs) — Data model: pool keys, spawn specs, thread options, the turn outcome, and the error type.

## What is shared and what is not

A pooled connection is shared by every task whose `AppServerKey` matches — same binary, same `CODEX_HOME`, same credential-bearing environment. Anything that would change *who the process authenticates as* is part of the key, never a per-thread override, because a process cannot hold two answers to that at once.

Everything per-task — cwd, model, sandbox, approval policy, the prompt — is a thread or turn parameter, which the protocol scopes correctly. Two threads in one process do not see each other's context.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.

The process-driving halves are covered end-to-end by `src/sdk/tests/e2e_codex_app_server.rs` against a scripted fake server, so a protocol change should be reflected in `src/sdk/tests/support/fake_app_server.rs` as well.
