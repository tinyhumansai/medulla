//! The ranked recent-sessions read model — [`list_recent_sessions`], the entry
//! point behind `medulla sessions` and the resume pane.
//!
//! It scans both harnesses' session files, dedupes by agent+id keeping the
//! freshest file, and orders the result current-folder-first then most-recent.

use std::collections::HashMap;

use super::scan::{
    claude_sessions_dir, codex_sessions_dir, collect_session_files, is_here, safe_resolve,
};
use super::summary::read_session_summary;
use super::types::{RawSessionFile, RecentSession, SessionAgentKind};

/// Default number of ranked sessions returned when no limit is given.
const DEFAULT_LIMIT: usize = 24;
/// Default number of newest files opened and parsed when no scan limit is given.
const DEFAULT_SCAN_LIMIT: usize = 60;

/// The most recent agent sessions across both harnesses, ordered
/// **current-folder-first, then most-recent**. Cost is bounded: only the newest
/// `scan_limit` files are opened, and only their first
/// [`HEAD_BYTES`](super::summary::HEAD_BYTES) parsed.
pub fn list_recent_sessions(
    env: &HashMap<String, String>,
    cwd: &str,
    limit: Option<usize>,
    scan_limit: Option<usize>,
) -> Vec<RecentSession> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let scan_limit = scan_limit.unwrap_or(DEFAULT_SCAN_LIMIT);

    let mut raw: Vec<RawSessionFile> = Vec::new();
    raw.extend(collect_session_files(
        SessionAgentKind::Claude,
        &claude_sessions_dir(env),
    ));
    raw.extend(collect_session_files(
        SessionAgentKind::Codex,
        &codex_sessions_dir(env),
    ));
    raw.sort_by_key(|file| std::cmp::Reverse(file.mtime_ms));
    raw.truncate(scan_limit);

    let here = safe_resolve(cwd);
    // Dedupe by agent+id, keeping the freshest file.
    let mut by_id: HashMap<String, RecentSession> = HashMap::new();
    for file in &raw {
        let summary = match read_session_summary(file.agent, &file.path) {
            Some(summary) => summary,
            None => continue,
        };
        let key = format!("{}:{}", file.agent.as_str(), summary.id);
        if let Some(existing) = by_id.get(&key) {
            if existing.last_active >= file.mtime_ms {
                continue;
            }
        }
        by_id.insert(
            key,
            RecentSession {
                agent: file.agent,
                id: summary.id,
                label: summary.label,
                last_active: file.mtime_ms,
                path: file.path.to_string_lossy().into_owned(),
                cwd: summary.cwd,
            },
        );
    }

    let mut sessions: Vec<RecentSession> = by_id.into_values().collect();
    sessions.sort_by(|left, right| {
        let left_here = if is_here(left.cwd.as_deref(), here.as_deref()) {
            0
        } else {
            1
        };
        let right_here = if is_here(right.cwd.as_deref(), here.as_deref()) {
            0
        } else {
            1
        };
        if left_here != right_here {
            return left_here.cmp(&right_here);
        }
        right.last_active.cmp(&left.last_active)
    });
    sessions.truncate(limit);
    sessions
}
