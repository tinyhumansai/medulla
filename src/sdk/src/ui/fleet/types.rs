//! Data model for the Fleet view: the node kind, the flattened tree row, and
//! their trivial classification impls. All behaviour lives in the sibling logic
//! modules.

/// Which level of the containment chain — or the template catalog beside it — a
/// row describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetNodeKind {
    /// A machine (`HostDescriptor`).
    Host,
    /// An agent CLI runtime installed on a host (`HarnessDescriptor`).
    Harness,
    /// A folder a harness exposes (`WorkspaceDescriptor`).
    Workspace,
    /// A durable agent identity deployed into a workspace, or pinned to a host
    /// when it has no workspace of its own.
    Agent,
    /// A declared kind of agent that may be provisioned (`AgentTemplate`).
    Template,
    /// A non-selectable section heading (`── templates ──`).
    Section,
}

impl FleetNodeKind {
    /// The display colour for this level. The chain darkens as it narrows —
    /// machine, runtime, folder, identity — so nesting reads at a glance even
    /// where the tree is too deep to indent further.
    pub fn color(self) -> &'static str {
        match self {
            FleetNodeKind::Host => "cyan",
            FleetNodeKind::Harness => "blue",
            FleetNodeKind::Workspace => "green",
            FleetNodeKind::Agent => "magenta",
            FleetNodeKind::Template => "yellow",
            FleetNodeKind::Section => "gray",
        }
    }

    /// The one-word label naming this level in the detail pane.
    pub fn label(self) -> &'static str {
        match self {
            FleetNodeKind::Host => "host",
            FleetNodeKind::Harness => "harness",
            FleetNodeKind::Workspace => "workspace",
            FleetNodeKind::Agent => "agent",
            FleetNodeKind::Template => "template",
            FleetNodeKind::Section => "section",
        }
    }

    /// Whether a row of this kind can be selected in the list. Headings cannot.
    pub fn selectable(self) -> bool {
        !matches!(self, FleetNodeKind::Section)
    }
}

/// One row of the flattened fleet tree.
///
/// The tree is pre-flattened rather than nested because the list widget scrolls
/// and selects over rows; `depth` carries the shape the nesting would have.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetNode {
    /// Selection key, `"<kind>:<id>"` — stable across refreshes so the cursor
    /// stays on the same host when a sibling appears or disappears.
    pub key: String,
    /// Which level of the chain this row describes.
    pub kind: FleetNodeKind,
    /// Indentation level, 0 at the host (or a top-level heading).
    pub depth: usize,
    /// The row's primary label: a name, a path, or a heading.
    pub label: String,
    /// The trailing status text: resources, readiness, budgets, availability.
    pub detail: String,
    /// Whether the row describes something declared unusable — an offline host,
    /// a harness that is not ready, an offline agent. Rendered dim.
    pub degraded: bool,
}
