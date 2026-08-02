# Codex

Codex flat-run mapper (`codex exec --json`): fold `event_msg` and `response_item` records into user_prompt, agent_message, agent_thinking, tool_call, tool_result, and status semantic events.

## Contents

- [`items.rs`](./items.rs) — The codex item fold: one `item.started` / `item.completed` record into semantic events.
- [`mod.rs`](./mod.rs) — Codex flat-run mapper (`codex exec --json`): fold `event_msg` and `response_item` records into user_prompt, agent_message, agent_thinking, tool_call, tool_result, and status semantic events.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
