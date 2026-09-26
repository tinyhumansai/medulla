# Providers

Provider detection + headless one-shot task execution, ported from the supported local harness providers.

## Contents

- [`acp.rs`](./acp.rs) — ACP-backed harness execution.
- [`codex_server/`](./codex_server) — Codex over a shared `codex app-server` process: one long-lived runtime serving a thread per task, selected by naming the `codex-server` harness.
- [`detect.rs`](./detect.rs) — Provider discovery and invocation shaping: which daemon providers exist, how to resolve a provider's binary, whether a provider accepts mid-run stdin, and how to build the one-shot headless argv for each provider.
- [`execute.rs`](./execute.rs) — Headless one-shot execution: spawn the provider CLI, stream its JSONL output through the shared semantic-event mappers to derive status updates and the final reply, enforce an idle watchdog + cooperative abort, and retry transient opencode SQLite-lock exits with jittered exponential backoff.
- [`mod.rs`](./mod.rs) — Provider detection + headless one-shot task execution, ported from the supported local harness providers.
- [`tests.rs`](./tests.rs) — Unit tests for provider detection, argv building, the run helpers, and the `Abort` handle. Moved verbatim from the former inline `#[cfg(test)] mod tests`; the wildcard `use super::*` is replaced with explicit imports because the logic now lives in sibling `detect`/`execute` modules.
- [`types.rs`](./types.rs) — Data model for headless provider runs: the callback aliases, the cooperative `Abort` handle, and the input/output records (`RunTaskOptions`, `RunTaskResult`) plus the injectable executor alias `RunTaskFn`. The detection and execution logic lives in the sibling `detect`/`execute` modules.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
