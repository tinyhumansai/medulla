//! Recent-session history, ported from the tinyplace CLI `session-history.ts`.
//!
//! The read model behind a "resume" pane: it scans the agents' own session dirs
//! (`~/.claude/projects`, `~/.codex/sessions`) — the same transcript files the
//! wrapper's tailer streams — so the list is always accurate with no separate
//! store. A row resolves to `{ agent, id }`, relaunched via
//! `claude --resume <id>` / `codex resume <id>` in the session's original cwd.
//! Exposed to the CLI via `medulla sessions` (JSON); a TUI picker lands later.
//!
//! The module is split by responsibility: [`types`] holds the data model,
//! [`scan`] locates and enumerates session files (and the wrapper's discovery
//! helpers), [`summary`] reads a file's head into an id/cwd/label, and [`list`]
//! ranks them into the public [`RecentSession`] read model. Public items are
//! re-exported here so callers use `medulla::session_history::*`.

mod list;
mod scan;
mod summary;
mod types;

#[cfg(test)]
mod tests;

pub use list::list_recent_sessions;
pub use scan::{claude_sessions_dir, codex_sessions_dir};
pub use types::{RecentSession, SessionAgentKind};

pub(crate) use scan::{collect_session_files, discover_session_file, preexisting_session_files};
