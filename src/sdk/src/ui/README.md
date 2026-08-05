# Ui

UI-facing data surface shared with the terminal app: `events` (the folded event log + `TuiEvent`), `agents` lane folding, `stream` token/thread derivations, the `chat_store`, the `work` panel over a harness's own todos and sub-agents, and small `util` helpers. Rendering-heavy screens (app, login, composer, theme) and the interactive onboarding screen live in the `medulla-tui` crate, which re-exports these data modules.

## Contents

- [`agents/`](./agents/) — Pure view-model fold: turn the flat event stream into one lane per cognitive tier plus one lane per connected roster agent / anonymous task / peer session, with a row model for the Agents list and pre-wrapped transcript lines. A port of the TS `deriveAgentLanes` / `agentRowModel` / `laneLines` essentials.
- [`chat_store/`](./chat_store/) — On-disk chat persistence for the Chat tab's thread trees.
- [`command/`](./command/) — Slash-command parsing, the command catalog, and the `/copy` transcript helper.
- [`decisions/`](./decisions/) — Prepared operator decisions derived from agent escalations and pending worker questions. The fold is UI-agnostic so terminal and future hosts share stable ids, ordering, deduplication, and answer routing.
- [`events/`](./events/) — The TUI event vocabulary: every library `CycleEvent` plus the host-sourced rows (cycle framing, conversation turns, agent/session status, effects). `TuiEvent` deserializes any JSON `{kind, ...}` shape, keeping unknown kinds as a passthrough so a newer backend never drops rows on an older TUI.
- [`fleet/`](./fleet/) — Pure view-model for the Fleet view: turn the declared capacity (`Host → Harness → Workspace → Agent`) and the agent-template catalog into a flattened row model plus pre-wrapped detail lines.
- [`harness/`](./harness/) — Read-only view-model helpers for the agent-harness contract: a compact task board rendering for a `HarnessStatus` payload, and a one-line budget note for an agent's `AgentBudgetMetadata` seat stamp. Pure formatting only — the `medulla-tui` crate turns the returned `Line`s / strings into ratatui spans.
- [`stream/`](./stream/) — Pure derivations over a `RuntimeSnapshot`'s event and thread streams.
- [`work/`](./work/) — Rendering a `WorkSnapshot` as display rows: the goal, the todo list, the sub-agents, the files touched, and how the run ended.
- [`workflows/`](./workflows/) — The UI-facing view of installed workflows: their listings, their graphs, and their runs.
- [`workspaces/`](./workspaces/) — The workspaces surface: every directory the fleet can work in, as one list.
- [`meters_tests.rs`](./meters_tests.rs) — Unit tests for the compact meters: bar fill, pressure colouring, the omit-rather-than-zero rule, and the lane usage accumulation.
- [`meters.rs`](./meters.rs) — Compact single-line meters: a labelled bar plus its numbers, sized to fit a header row rather than a dashboard.
- [`mod.rs`](./mod.rs) — UI-facing data surface shared with the terminal app: `events` (the folded event log + `TuiEvent`), `agents` lane folding, `stream` token/thread derivations, the `chat_store`, the `work` panel over a harness's own todos and sub-agents, and small `util` helpers. Rendering-heavy screens (app, login, composer, theme) and the interactive onboarding screen live in the `medulla-tui` crate, which re-exports these data modules.
- [`util_tests.rs`](./util_tests.rs) — Tests for the util module.
- [`util.rs`](./util.rs) — Small display helpers shared by the views.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
