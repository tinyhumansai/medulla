//! Filesystem scanning and discovery: locating each agent's session directory,
//! enumerating the transcript files inside it, and finding the newest file that
//! belongs to a just-launched session.
//!
//! Directory resolution defers to the central env resolver in
//! [`crate::tinyplace`]; the rest walks the tree, matches session-file
//! names, and reads mtimes. Path helpers ([`safe_resolve`], [`is_here`]) are
//! shared with the ranking logic in [`super::list`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::summary::read_session_summary;
use super::types::{DiscoveredSession, RawSessionFile, SessionAgentKind};

/// Resolve the Claude session directory via the central env resolver.
pub fn claude_sessions_dir(env: &HashMap<String, String>) -> PathBuf {
    crate::tinyplace::env::sessions_dir(crate::tinyplace::HarnessProvider::Claude, env)
}

/// Resolve the Codex session directory via the central env resolver.
pub fn codex_sessions_dir(env: &HashMap<String, String>) -> PathBuf {
    crate::tinyplace::env::sessions_dir(crate::tinyplace::HarnessProvider::Codex, env)
}

/// Enumerate every session file under `dir` for `agent`, recursing into
/// subdirectories. An absent directory yields an empty list.
pub(super) fn collect_session_files(agent: SessionAgentKind, dir: &Path) -> Vec<RawSessionFile> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    visit(agent, dir, &mut out);
    out
}

/// Recursively walk `directory`, pushing every matching session file (with its
/// mtime) into `out`. Unreadable entries are skipped rather than failing.
fn visit(agent: SessionAgentKind, directory: &Path, out: &mut Vec<RawSessionFile>) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            visit(agent, &path, out);
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !file_type.is_file() || !is_session_file(agent, &path, &name) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            out.push(RawSessionFile {
                agent,
                path,
                mtime_ms,
            });
        }
    }
}

/// Whether `path`/`name` is a real session transcript for `agent`: codex files
/// are `rollout-*.jsonl`; claude files are any `.jsonl` outside a `subagents/`
/// directory.
pub(super) fn is_session_file(agent: SessionAgentKind, path: &Path, name: &str) -> bool {
    match agent {
        SessionAgentKind::Codex => name.starts_with("rollout-") && name.ends_with(".jsonl"),
        SessionAgentKind::Claude => {
            let sep = std::path::MAIN_SEPARATOR;
            let subagents = format!("{sep}subagents{sep}");
            name.ends_with(".jsonl") && !path.to_string_lossy().contains(&subagents)
        }
    }
}

/// The session directory for a given agent kind, honoring env overrides.
/// Used by the wrapper's tailer to locate live transcripts. (opencode is
/// intentionally not covered — its wrapper uses an SSE bridge, a scope cut.)
pub(crate) fn sessions_dir_for(env: &HashMap<String, String>, agent: SessionAgentKind) -> PathBuf {
    match agent {
        SessionAgentKind::Claude => claude_sessions_dir(env),
        SessionAgentKind::Codex => codex_sessions_dir(env),
    }
}

/// The set of session files that already exist for `agent` (canonicalized paths).
/// The wrapper records this at start so it can ignore pre-existing transcripts and
/// only latch onto the session its own child creates.
pub(crate) fn preexisting_session_files(
    env: &HashMap<String, String>,
    agent: SessionAgentKind,
) -> HashSet<PathBuf> {
    collect_session_files(agent, &sessions_dir_for(env, agent))
        .into_iter()
        .filter_map(|file| std::fs::canonicalize(&file.path).ok())
        .collect()
}

/// Find the newest session file for `agent` anchored at `cwd`, ignoring any file
/// in `ignored` and any older than `min_mtime_ms`. Mirrors the TS wrapper's
/// `locateSession`: newest-first, `meta.cwd == cwd`, skipping pre-existing files.
pub(crate) fn discover_newest_session_file(
    env: &HashMap<String, String>,
    agent: SessionAgentKind,
    cwd: &str,
    min_mtime_ms: i64,
    ignored: &HashSet<PathBuf>,
) -> Option<DiscoveredSession> {
    let here = safe_resolve(cwd);
    let mut files = collect_session_files(agent, &sessions_dir_for(env, agent));
    files.sort_by_key(|file| std::cmp::Reverse(file.mtime_ms));
    for file in files {
        if file.mtime_ms < min_mtime_ms {
            continue;
        }
        let canonical = std::fs::canonicalize(&file.path).unwrap_or_else(|_| file.path.clone());
        if ignored.contains(&canonical) {
            continue;
        }
        let summary = match read_session_summary(agent, &file.path) {
            Some(summary) => summary,
            None => continue,
        };
        // A session with a recorded cwd must match; one with no cwd is accepted
        // (some transcripts omit it in their head window).
        if let Some(session_cwd) = &summary.cwd {
            if safe_resolve(session_cwd) != here {
                continue;
            }
        }
        return Some(DiscoveredSession {
            path: canonical,
            id: summary.id,
            cwd: summary.cwd,
        });
    }
    None
}

/// Whether a session's recorded `cwd` resolves to the same path as `here`.
/// Both sides must be present for a match.
pub(super) fn is_here(cwd: Option<&str>, here: Option<&str>) -> bool {
    match (cwd, here) {
        (Some(cwd), Some(here)) => safe_resolve(cwd).as_deref() == Some(here),
        _ => false,
    }
}

/// Canonicalize `path`, falling back to a lexical join against the current dir
/// when the path does not exist (matching the TS `resolve`, which never touches
/// the filesystem).
pub(super) fn safe_resolve(path: &str) -> Option<String> {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .ok()
        .or_else(|| {
            // A path that does not exist still resolves lexically to itself here,
            // matching the TS `resolve` (which never touches the filesystem).
            std::env::current_dir()
                .ok()
                .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
        })
}
