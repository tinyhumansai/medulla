//! JSONL line → semantic-event mappers, ported from the tinyplace CLI
//! provider output into the public harness-event contract.
//!
//! Each provider's coding-agent CLI streams a different JSONL transcript shape;
//! these three mappers fold a single raw line into zero or more
//! [`HarnessSemanticEvent`]s over the shared SDK event model
//! ([`HarnessEvent`](crate::protocol::HarnessEvent), re-exported by
//! `tinyplace-proto`). Only the flat run formats are ported here (`claude -p
//! --output-format stream-json`, `codex exec --json`, `opencode run --format
//! json`); the interactive opencode SSE bus mapper belongs to the PTY wrapper,
//! which lands separately.
//!
//! Split by responsibility: `types` holds the data model
//! ([`HarnessSemanticEvent`], [`HarnessLineMapper`]); `mapper` the stateful
//! per-stream fold and dedupe; `events` the shared event/payload constructors;
//! `claude`, `codex`, and `opencode` the per-provider line mappers;
//! `shared` the text/tool helpers and truncation caps; `timestamp` the
//! RFC3339 → epoch-ms parsing; `usage` the token-usage scan; and `work` the
//! recognizer that turns a structured tool call (a todo write, a plan, a
//! sub-agent spawn, a file edit) into the provider-neutral work events of
//! [`crate::harness_work`]. All public items are re-exported here so callers use
//! `medulla::daemon::mappers::*`.

mod claude;
mod codex;
mod events;
mod mapper;
mod opencode;
mod shared;
mod timestamp;
mod types;
mod usage;
mod work;
mod workspace;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_ext;
#[cfg(test)]
mod tests_work;
#[cfg(test)]
mod tests_workspace_context;

pub use shared::{normalize_tool_kind, tool_display};
pub use timestamp::parse_iso_ms;
pub use types::{HarnessLineMapper, HarnessSemanticEvent};
