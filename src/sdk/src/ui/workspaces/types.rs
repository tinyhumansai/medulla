//! Data types for the workspaces surface.

/// One directory the fleet can work in, as the Routing page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    /// Absolute path, as the owning machine sees it.
    pub path: String,
    /// What to call it: the declared project, else the directory's own name.
    pub label: String,
    /// The machine it lives on. `None` means this device.
    pub host: Option<String>,
    /// A short line of context — the workspace profile's first sentence, or the
    /// harness that exposes it. `None` when nothing is known beyond the path.
    pub detail: Option<String>,
    /// Whether this row is declared in *this* device's config, and so can be
    /// removed here. A workspace declared by a remote host is read-only: it is
    /// that machine's to change.
    pub editable: bool,
    /// Whether this is where delegated work actually runs on its machine.
    ///
    /// Exactly one row per host is primary. The others are advertised context —
    /// the orchestrator learns the machine has them, but a task frame cannot
    /// select one.
    pub primary: bool,
    /// Whether the directory is present on disk.
    ///
    /// Only meaningful for [`editable`](Self::editable) rows: a remote host's
    /// paths cannot be checked from here, so they are always reported present
    /// rather than falsely flagged missing.
    pub exists: bool,
}
