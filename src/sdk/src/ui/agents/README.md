# Agents

Pure view-model fold: turn the flat event stream into one lane per cognitive tier plus one lane per connected roster agent / anonymous task / peer session, with a row model for the Agents list and pre-wrapped transcript lines. A port of the TS `deriveAgentLanes` / `agentRowModel` / `laneLines` essentials.

## Contents

- [`lanes/`](./lanes/) — The event-stream fold: turn the flat event log into the orchestrator lane plus one lane per connected roster agent / anonymous task / peer session. A port of the TS `deriveAgentLanes` essentials. Owns `derive_agent_lanes` and the private lane-collection machinery it drives.
- [`tests/`](./tests/) — Unit tests for the Agents view-model, split by responsibility: `fold` covers the event fold and Agents-list row model; `render` covers status/role classification, key parsing, and transcript rendering; `roster` covers the worker-registry merge that feeds the fold.
- [`activity.rs`](./activity.rs) — Fold locally-observed worker activity into the Agents-view lanes.
- [`fmt.rs`](./fmt.rs) — Small formatting helpers shared by the fold: event-kind colours and the header/tool-call string builders. Kept separate so the lane fold in `super::lanes` stays focused on the state machine rather than string plumbing.
- [`keys.rs`](./keys.rs) — Lane-key parsing: recover the wire `(cycleId, taskId)` from a composed lane key.
- [`lines.rs`](./lines.rs) — Transcript rendering: flatten a lane's or task's folded turns into pre-wrapped, styled `Line` rows for the detail pane. Owns `lane_lines` and `task_lines` and the shared block-to-lines walker they both use.
- [`mod.rs`](./mod.rs) — Pure view-model fold: turn the flat event stream into one lane per cognitive tier plus one lane per connected roster agent / anonymous task / peer session, with a row model for the Agents list and pre-wrapped transcript lines. A port of the TS `deriveAgentLanes` / `agentRowModel` / `laneLines` essentials.
- [`roster.rs`](./roster.rs) — Merge the local worker registry into the Agents-view roster.
- [`rows.rs`](./rows.rs) — Agents-list row derivation: ordering a lane's tasks and flattening the lanes into the printable `AgentRow` sequence (lane headers, the functions divider, and capped per-task sublanes).
- [`types.rs`](./types.rs) — Data model for the Agents view-model: the lane/task/turn structs, the role and status enums with their trivial classification impls, the pre-styled display `Line`, and the `AgentRow` list-row enum. All behaviour lives in the sibling logic modules; this file holds only the shapes and their trivial accessors.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
