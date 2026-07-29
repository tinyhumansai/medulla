# Daemon

The headless `medulla daemon`: offer this machine's local coding-agent CLIs (Claude Code / Codex / OpenCode) as an addressable tiny.place agent over Signal end-to-end encrypted DMs, speaking both plain-text prompts and the `medulla-tinyplace/1` task protocol an orchestrator delegates with.

## Contents

- [`capabilities/`](./capabilities/) — Capability discovery for supported local harnesses.
- [`dir_context/`](./dir_context/) — Workspace directory context for the capability probe.
- [`embedded/`](./embedded/) — The embedded host: a `DaemonRuntime` driven over any `Bridge`, inside someone else's process.
- [`flags/`](./flags/) — Command-line flag parsing for `medulla daemon`: the permissive `Flags` tokenizer (values, repeatable comma-lists, and boolean switches) and `parse_provider`, the wire-name → `HarnessProvider` mapper. Consumed by `super::entry` to build the daemon configuration.
- [`listener/`](./listener/) — Envelopes delivered over the relay's push channel, instead of fetched.
- [`mappers/`](./mappers/) — JSONL line → semantic-event mappers, ported from the tinyplace CLI provider output into the public harness-event contract.
- [`pairing/`](./pairing/) — Pairing hand-off: getting a worker's address from the machine it runs on to the orchestrator that will delegate to it.
- [`providers/`](./providers/) — Provider detection + headless one-shot task execution, ported from the supported local harness providers.
- [`task_loop/`](./task_loop/) — The frame- and task-handling half of `DaemonRuntime`, split by what each kind of frame asks for so no file exceeds the repo's 500-line ceiling: `probe` answers the cached capability probe, `system_info` reports cheap host capacity, `control` delivers mid-run input and stops a task the requester has given up on, and `run` executes a task with its slot limit, throttled status forwarding, and plain-text fallback.
- [`tests/`](./tests/) — Runtime state-machine tests driven by a fake executor (no network, no CLIs), split by surface: `task_tests` covers task acceptance, dispatch, input forwarding, and shutdown; `provider_tests` covers provider selection and plain-text DM routing; `capability_tests` covers the cached capability probe, status throttling, and semantic-event → status-line mapping.
- [`transport/`](./transport/) — Encrypted Signal DM transport for the daemon.
- [`entry_tests.rs`](./entry_tests.rs) — Tests for the entry module.
- [`entry.rs`](./entry.rs) — The `medulla daemon` CLI entry point: `run_daemon` wires provider detection, identity/config bootstrap, tiny.place onboarding, and the transport-backed serve loop around a `DaemonRuntime`. Flag parsing lives in `super::flags`; the runtime state machine in `super::runtime` and `super::task_loop`.
- [`mod.rs`](./mod.rs) — The headless `medulla daemon`: offer this machine's local coding-agent CLIs (Claude Code / Codex / OpenCode) as an addressable tiny.place agent over Signal end-to-end encrypted DMs, speaking both plain-text prompts and the `medulla-tinyplace/1` task protocol an orchestrator delegates with.
- [`runtime.rs`](./runtime.rs) — `DaemonRuntime` lifecycle: construction and test overrides, fire-and-forget dispatch and idle/shutdown coordination, controller bookkeeping, and the encrypted reply helpers. The frame- and task-handling half of the state machine lives in `super::task_loop`.
- [`status.rs`](./status.rs) — Status-line derivation: turn a semantic harness event into the short, human-facing detail string the daemon forwards as a `status` frame. Ported from provider status details, and extended with the work-derived line the newer structured events need.
- [`types.rs`](./types.rs) — Daemon data model: the callback type aliases, non-callback `DaemonConfig`, the shared per-runtime `Inner` state, and the cheaply-clonable `DaemonRuntime` handle.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
