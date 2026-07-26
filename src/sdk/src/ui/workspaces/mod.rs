//! The workspaces surface: every directory the fleet can work in, as one list.
//!
//! Two things feed it, and the distinction matters to the operator. This
//! device's directories are declared locally in `[host]` and are editable here.
//! Every other machine's are declared by that machine and arrive in the
//! [`CapacitySnapshot`] — visible, so the orchestrator's picture and the
//! operator's agree, but not editable from this end.
//!
//! Why the page exists at all: an orchestrator that knows only "there is a
//! claude daemon on this laptop" cannot decide *what to work on*. It needs to
//! know the machine has a Rust workspace here and a backend there. These rows
//! are the operator's view of exactly the list that reaches it as
//! `capabilities.accessibleDirs`.

use crate::config::HostSection;
use crate::runtime::fleet::CapacitySnapshot;

mod types;
pub use types::WorkspaceRow;

#[cfg(test)]
mod tests;

/// Build the workspace list for the Routing page.
///
/// `host` supplies this device's declared directories, `capacity` the rest of
/// the fleet's. `primary` is this device's resolved working directory — the
/// config's value is often blank (meaning "wherever medulla was launched"), so
/// the caller passes the resolved one rather than this function guessing.
///
/// A fleet workspace whose path this device already declares is dropped rather
/// than listed twice: the same directory named by two sources is one place to
/// work, and the local row is the one that can be edited.
pub fn workspace_rows(
    host: &HostSection,
    primary: &str,
    capacity: &CapacitySnapshot,
) -> Vec<WorkspaceRow> {
    let mut rows: Vec<WorkspaceRow> = Vec::new();
    let primary = primary.trim();
    if !primary.is_empty() {
        rows.push(local_row(primary, true));
    }
    for path in &host.workspaces {
        let path = path.trim();
        if path.is_empty() || rows.iter().any(|r| r.path == path) {
            continue;
        }
        rows.push(local_row(path, false));
    }

    for workspace in &capacity.workspaces {
        let path = workspace.path.trim();
        if path.is_empty() {
            continue;
        }
        // Walk up to the machine so the row says *where*, not just *what*. A
        // dangling harness link leaves the host unnamed rather than dropping the
        // workspace: a directory nobody can place is still a directory.
        let harness = capacity.harness(&workspace.harness_id);
        let host_name = harness
            .and_then(|h| capacity.host(&h.host_id))
            .map(|h| h.name.clone())
            .or_else(|| harness.map(|h| h.host_id.clone()));
        // Deduplicate by machine *and* path, not path alone. Paths are local to
        // the machine that reported them, so `/workspace/repo` on two hosts is
        // two distinct places to send work — collapsing them hides real capacity,
        // and container fleets make identical paths the norm rather than a
        // curiosity. A remote row is still folded into a local one at the same
        // path, because that genuinely is one directory seen twice.
        if rows
            .iter()
            .any(|r| r.path == path && (r.host.is_none() || r.host == host_name))
        {
            continue;
        }
        let detail = workspace
            .profile
            .as_ref()
            .map(|p| p.instructions.as_str())
            .and_then(first_line)
            .or_else(|| harness.map(|h| h.kind.clone()));
        rows.push(WorkspaceRow {
            path: path.to_string(),
            label: label_for(workspace.project.as_deref(), &workspace.name, path),
            host: host_name,
            detail,
            editable: false,
            // Declared capacity does not distinguish a machine's default
            // directory, and guessing one would put a claim on the screen that
            // nothing backs.
            primary: false,
            exists: true,
        });
    }
    rows
}

/// A row for a directory declared on this device.
fn local_row(path: &str, primary: bool) -> WorkspaceRow {
    WorkspaceRow {
        path: path.to_string(),
        label: label_for(None, "", path),
        host: None,
        detail: None,
        editable: true,
        primary,
        exists: std::path::Path::new(path).is_dir(),
    }
}

/// What to call a workspace: its declared project, else its declared name, else
/// the last path component. Never the whole path — the path is already the row.
fn label_for(project: Option<&str>, name: &str, path: &str) -> String {
    for candidate in [project.unwrap_or(""), name] {
        let candidate = candidate.trim();
        if !candidate.is_empty() {
            return candidate.to_string();
        }
    }
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| path.to_string())
}

/// The first non-empty line of a profile, for the one-line detail column.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}
