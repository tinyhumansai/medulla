//! Pure session-listing helper for `main`: the `medulla sessions` payload
//! ([`sessions_json`]). Kept free of socket and terminal I/O so it stays
//! unit-testable without a TTY.

use std::collections::HashMap;

use medulla::session_history::list_recent_sessions;

/// The `medulla sessions` payload: recent claude/codex sessions as pretty JSON.
pub fn sessions_json(env: &HashMap<String, String>, cwd: &str) -> anyhow::Result<String> {
    let sessions = list_recent_sessions(env, cwd, None, None);
    Ok(serde_json::to_string_pretty(&sessions)?)
}
