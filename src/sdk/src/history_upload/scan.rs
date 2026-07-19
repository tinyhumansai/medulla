//! Locating the transcripts that will be shared, and reading them off disk.
//!
//! Directory discovery reuses [`crate::session_history`]'s scanner — the same
//! files the resume pane lists — rather than re-deriving where each agent keeps
//! its history. This module adds only what sharing needs: size accounting, the
//! caps that bound an upload, and reading a whole file (not just its head).

use std::collections::HashMap;

use crate::session_history::{
    claude_sessions_dir, codex_sessions_dir, collect_session_files, SessionAgentKind,
};

use super::redact::redact_text;
use super::types::{HistoryScan, HistorySessionFile, RedactedSession};

/// Most transcripts uploaded in one claim. Mirrors the backend's own cap; the
/// newest sessions are kept, since they best represent current usage.
pub const MAX_UPLOAD_SESSIONS: usize = 200;

/// Largest transcript accepted, matching the backend's per-file limit. Bigger
/// files are skipped rather than truncated so a partial transcript never
/// misreports a user's usage.
pub const MAX_SESSION_BYTES: u64 = 10 * 1024 * 1024;

/// The agents whose history Phase 1 can read from disk.
///
/// OpenCode is absent deliberately: it stores sessions in a SQLite database
/// rather than JSONL transcripts, so it needs a different reader than this
/// file-scanning path provides.
const SCANNED_AGENTS: [SessionAgentKind; 2] = [SessionAgentKind::Claude, SessionAgentKind::Codex];

/// Finds every shareable transcript on this machine, newest first and capped.
///
/// Purely a read model — nothing is opened or uploaded here, so it is cheap
/// enough to drive the consent screen's "we found N sessions (~X MB)" summary.
pub fn scan_local_history(env: &HashMap<String, String>) -> HistoryScan {
    let mut files: Vec<HistorySessionFile> = Vec::new();
    let mut skipped_oversize = 0usize;

    for agent in SCANNED_AGENTS {
        let dir = match agent {
            SessionAgentKind::Claude => claude_sessions_dir(env),
            SessionAgentKind::Codex => codex_sessions_dir(env),
        };
        for raw in collect_session_files(agent, &dir) {
            let size_bytes = std::fs::metadata(&raw.path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            if size_bytes == 0 {
                continue;
            }
            if size_bytes > MAX_SESSION_BYTES {
                skipped_oversize += 1;
                continue;
            }
            files.push(HistorySessionFile {
                agent: raw.agent,
                path: raw.path,
                size_bytes,
                mtime_ms: raw.mtime_ms,
            });
        }
    }

    files.sort_by_key(|file| std::cmp::Reverse(file.mtime_ms));
    let skipped_over_cap = files.len().saturating_sub(MAX_UPLOAD_SESSIONS);
    files.truncate(MAX_UPLOAD_SESSIONS);

    HistoryScan {
        files,
        skipped_oversize,
        skipped_over_cap,
    }
}

/// Reads one transcript and scrubs its secrets, ready for upload.
///
/// Returns `None` when the file cannot be read or is not valid UTF-8 — a single
/// unreadable transcript must never abort the whole flow.
pub fn read_redacted_session(file: &HistorySessionFile) -> Option<RedactedSession> {
    let raw = std::fs::read(&file.path).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let (content, redactions) = redact_text(&text);

    Some(RedactedSession {
        agent: file.agent,
        path: file.path.clone(),
        content,
        redactions,
    })
}
