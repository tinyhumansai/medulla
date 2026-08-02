# Mappers

JSONL line → semantic-event mappers, ported from the tinyplace CLI provider output into the public harness-event contract.

## Contents

- [`codex/`](./codex/) — Codex flat-run mapper (`codex exec --json`): fold `event_msg` and `response_item` records into user_prompt, agent_message, agent_thinking, tool_call, tool_result, and status semantic events.
- [`claude.rs`](./claude.rs) — Claude flat-run mapper (`claude -p --output-format stream-json`): fold a user or assistant transcript record into user_prompt, agent_message, agent_thinking, tool_call, and tool_result semantic events, plus the work events its `system`, `result`, and structured tool records imply.
- [`events.rs`](./events.rs) — Event construction helpers shared by the three provider mappers: build a `HarnessEvent`/`HarnessSemanticEvent` and the tool_call / tool_result payload envelopes over the shared SDK event model.
- [`mapper.rs`](./mapper.rs) — The stateful per-stream fold: `HarnessLineMapper`'s constructor, usage accessor, and the per-line dispatch to the provider mappers, including the codex duplicate-message dedupe and the token-usage scan.
- [`mod.rs`](./mod.rs) — JSONL line → semantic-event mappers, ported from the tinyplace CLI provider output into the public harness-event contract.
- [`opencode.rs`](./opencode.rs) — OpenCode flat-run mapper (`opencode run --format json`): fold `error`, `text`, `reasoning`, and `tool` part records into error, agent_message, agent_thinking, tool_call, and tool_result semantic events.
- [`shared.rs`](./shared.rs) — Provider-agnostic text and tool helpers plus the truncation caps: tool-name normalization, one-line call summaries, byte-capped truncation, structured input bounding, and the small JSON-shape utilities the three provider mappers share.
- [`tests_ext.rs`](./tests_ext.rs) — Additional branch-coverage tests for the JSONL line mappers: opencode/codex error and tool shapes, the claude content-block folds, the shared JSON/text helpers, and the RFC3339 fraction/offset edges. Split from `tests.rs` to keep each test file under the module size ceiling.
- [`tests_work.rs`](./tests_work.rs) — Tests for the work recognizer: the todo lists, plans, sub-agents, file edits, and session facts each provider's transcript carries, and the folded snapshot they add up to.
- [`tests.rs`](./tests.rs) — Unit tests for the JSONL line mappers: the token-usage scan, the per-provider folds (claude/codex/opencode), the codex dedupe, the shared tool helpers, and the RFC3339 timestamp parser.
- [`timestamp.rs`](./timestamp.rs) — Timestamp parsing: fold a transcript record's ISO-8601 string into epoch milliseconds, with a dependency-free RFC3339 parser and a receive-time fallback so a live session's derived status clock never reads as stale.
- [`types.rs`](./types.rs) — Data model for the JSONL line mappers: the pre-envelope semantic event and the stateful per-stream mapper's fields. The mapper's fold behavior lives in `mapper`; the fields are `pub(super)` so it can drive them.
- [`usage.rs`](./usage.rs) — Token-usage extraction: a depth-bounded scan that finds the input/output token counts wherever a provider nests them on its records.
- [`work.rs`](./work.rs) — Recognizing the *work* inside a tool call.

## Maintenance

Keep this index synchronized when responsibilities move. Put shared data structures in `types.rs`, focused unit tests in `tests.rs` or a sibling `_tests.rs`, and preserve the module-level Rust documentation as the API source of truth.
