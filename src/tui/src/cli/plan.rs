//! Pure runtime-selection and session-listing helpers for `main`: the
//! `medulla sessions` payload ([`sessions_json`]) and the core-socket decision
//! ([`resolve_socket_path`] / [`core_socket_plan`]). Kept free of socket and
//! terminal I/O so it is unit-testable and, deliberately, cross-platform (it
//! compiles on Windows, unlike the unix-only `runtime::core_client`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use medulla::session_history::list_recent_sessions;

use super::types::CorePlan;

/// The `medulla sessions` payload: recent claude/codex sessions as pretty JSON.
pub fn sessions_json(env: &HashMap<String, String>, cwd: &str) -> anyhow::Result<String> {
    let sessions = list_recent_sessions(env, cwd, None, None);
    Ok(serde_json::to_string_pretty(&sessions)?)
}

/// Resolve the core socket path (§1.1). An explicit `override_path` (the `--core`
/// flag or `[core].socketPath` config) wins; otherwise `$XDG_RUNTIME_DIR/medulla/
/// core.sock`, then `<state_dir>/core.sock`. `None` when nothing is available.
///
/// Pure path logic with no socket API, so it lives here (a cross-platform module)
/// rather than in the unix-only `runtime::core_client`; that keeps
/// [`core_socket_plan`] compiling on Windows.
pub fn resolve_socket_path(
    override_path: Option<&str>,
    runtime_dir: Option<&str>,
    state_dir: Option<&str>,
) -> Option<PathBuf> {
    if let Some(p) = override_path.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(dir) = runtime_dir.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join("medulla").join("core.sock"));
    }
    if let Some(dir) = state_dir.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join("core.sock"));
    }
    None
}

/// Decide whether to attempt the core runtime, resolving and probing its socket.
/// `exists` probes a path (injected so this stays pure and testable).
pub fn core_socket_plan(
    want_core: bool,
    config_socket: Option<&str>,
    runtime_dir: Option<&str>,
    state_dir: Option<&str>,
    exists: impl Fn(&Path) -> bool,
) -> CorePlan {
    if !want_core {
        return CorePlan::Skip;
    }
    match resolve_socket_path(config_socket, runtime_dir, state_dir) {
        Some(path) if exists(&path) => CorePlan::Connect(path),
        Some(path) => {
            CorePlan::Fallback(format!("core socket {} not present — falling back", path.display()))
        }
        None => CorePlan::Fallback(
            "no core socket resolved (set XDG_RUNTIME_DIR / MEDULLA_STATE_DIR / [core].socketPath) — falling back"
                .into(),
        ),
    }
}
