//! Resolves immutable launch baselines for live harness sessions.

use std::path::Path;

use crate::worker::pty::SessionRow;

use super::repository;

/// Return the immutable launch baseline, deriving Git's empty tree when the
/// harness started before an unborn repository had its first commit.
pub(super) fn launch_baseline(
    cwd: &str,
    launch_root: Option<&str>,
    launch_commit: Option<&str>,
    launch_checkout_identity: Option<&str>,
) -> Option<String> {
    let launch_root = Path::new(launch_root?);
    let (current_root, _) = repository::discover_in(Path::new(cwd)).ok()?;
    if current_root != launch_root {
        return None;
    }
    if !crate::worker::pty::checkout::matches(Path::new(cwd), launch_checkout_identity?) {
        return None;
    }
    let (root, _) = repository::discover_in(launch_root).ok()?;
    match launch_commit {
        Some(commit) => repository::resolve_commit(&root, commit).ok(),
        None => repository::empty_tree(&root).ok(),
    }
}

/// Resolve the selected harness without silently substituting another
/// repository. The newest eligible harness is only a default when no live
/// preferred row exists.
pub(super) fn select_harness_baseline(
    mut rows: Vec<SessionRow>,
    preferred_id: Option<&str>,
) -> Result<Option<(SessionRow, String)>, String> {
    if let Some(preferred) = preferred_id {
        if let Some(row) = rows.iter().find(|row| row.id == preferred).cloned() {
            let commit = launch_baseline(
                &row.cwd,
                row.launch_root.as_deref(),
                row.launch_commit.as_deref(),
                row.launch_checkout_identity.as_deref(),
            )
            .ok_or_else(|| format!("Selected harness {} is not in a Git repository", row.label))?;
            return Ok(Some((row, commit)));
        }
    }

    // Validate from newest to oldest and stop at the first usable repository;
    // Git discovery launches subprocesses and must not scale with the fan-out.
    rows.sort_unstable_by_key(|row| std::cmp::Reverse(row.started_at));
    Ok(rows.into_iter().find_map(|row| {
        launch_baseline(
            &row.cwd,
            row.launch_root.as_deref(),
            row.launch_commit.as_deref(),
            row.launch_checkout_identity.as_deref(),
        )
        .map(|commit| (row, commit))
    }))
}
