# Runtime

The `Runtime` trait the UI drives, plus its snapshot contract. Concrete implementations live alongside: `openhuman` (the embedded core, which the product runs on) and `mock` (tests and demos). The UI depends only on the trait and its types.

## Contents

- [`capabilities/`](./capabilities/) — Narrow capability interfaces over the compatibility-facing `Runtime`.
- [`event_log/`](./event_log/) — Shared bounded event storage for runtime conversation threads.
- [`fleet/`](./fleet/) — The declared-capacity contracts: the strict single-parent containment chain `Host → Harness → Workspace → Agent`, the agent-template catalog that constrains what may be provisioned into it, and the `CapacitySnapshot` roll-up the UI renders.
- [`headless/`](./headless/) — A non-interactive driver over the `Runtime` trait for scripting and end-to-end automation (a docker container, a CI probe): attach a runtime, submit exactly one instruction, stream the folded events to a writer as JSON lines, and return once the cycle result lands.
- [`mock/`](./mock/) — A scripted, self-contained `Runtime` used by `main` until the backend runtime lands, and by tests. It fabricates a plausible event stream so every tab has something to render.
- [`openhuman/`](./openhuman/) — `Runtime` backed by the embedded OpenHuman core.
- [`tests/`](./tests/) — Unit tests for the runtime trait surface's helper types — chiefly `WorkerOp` parsing and the snapshot defaults.
- [`mod.rs`](./mod.rs) — The `Runtime` trait the UI drives, plus its snapshot contract. Concrete implementations live alongside: `openhuman` (the embedded core, which the product runs on) and `mock` (tests and demos). The UI depends only on the trait and its types.
- [`types.rs`](./types.rs) — Shared data types for the runtime abstraction.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
