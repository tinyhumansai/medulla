# Lanes

The event-stream fold: turn the flat event log into the orchestrator lane plus one lane per connected roster agent / anonymous task / peer session. A port of the TS `deriveAgentLanes` essentials. Owns `derive_agent_lanes` and the private lane-collection machinery it drives.

## Contents

- [`mod.rs`](./mod.rs) — The event-stream fold: turn the flat event log into the orchestrator lane plus one lane per connected roster agent / anonymous task / peer session. A port of the TS `deriveAgentLanes` essentials. Owns `derive_agent_lanes` and the private lane-collection machinery it drives.
- [`types.rs`](./types.rs) — Data types for the `lanes` module.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
