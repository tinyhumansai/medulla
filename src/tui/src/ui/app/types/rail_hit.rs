//! Compact, identity-bearing pointer targets for the rendered Sessions rail.

/// The compact pointer action a rendered rail line represents.
///
/// This deliberately excludes the rendered [`RailRow`](super::super::rail::RailRow):
/// sessions can retain their transcript, and a per-line hit map must not clone
/// that transcript for every wrapped or off-screen row.
#[derive(Clone)]
pub(in crate::ui::app) enum RailHitTarget {
    /// A non-selectable label or host row.
    Inert,
    /// A selectable row with no direct pointer action.
    Selectable,
    /// An action row that opens the session picker.
    ///
    /// Carries nothing: the picker asks for the harness and the directory
    /// itself, so which `+` row was clicked no longer changes what happens.
    NewSession,
    /// The action that pages an agent lane's tasks.
    Overflow,
    /// A row attached to this local harness session.
    Session(String),
    /// A workflow run whose graph view should open.
    WorkflowRun {
        /// Stable workflow identifier.
        workflow_id: String,
        /// Stable run identifier within the workflow.
        run_id: String,
    },
}

impl RailHitTarget {
    /// The local harness session this target names, if it names one.
    pub(in crate::ui::app) fn session_id(&self) -> Option<String> {
        match self {
            Self::Session(session) => Some(session.clone()),
            _ => None,
        }
    }
}

/// One rendered Sessions-rail line and the stable cursor state it represented.
///
/// Pointer input happens after drawing, when live lanes may have changed. The
/// hit map therefore retains the row and its anchor from that frame instead of
/// treating a rendered offset as an offset into a new rail projection.
#[derive(Clone)]
pub(in crate::ui::app) struct RailHit {
    /// The compact action selected by this drawn line.
    pub(in crate::ui::app) target: RailHitTarget,
    /// Test-only copy of the row so focused interaction tests can name it.
    #[cfg(test)]
    pub(in crate::ui::app) row: super::super::rail::RailRow,
    /// The durable cursor identity resolved while the row was rendered.
    pub(in crate::ui::app) anchor: Option<super::super::rail::RailAnchor>,
    /// The row's rendered offset, retained only as a fallback if it has no anchor.
    pub(in crate::ui::app) index: usize,
}

impl RailHit {
    /// Capture just the data pointer routing needs from a rendered rail row.
    pub(in crate::ui::app) fn from_row(
        row: &super::super::rail::RailRow,
        anchor: Option<super::super::rail::RailAnchor>,
        index: usize,
    ) -> Self {
        use super::super::rail::RailRow;

        let target = if row.is_new_session() {
            RailHitTarget::NewSession
        } else if matches!(row, RailRow::Overflow { .. }) {
            RailHitTarget::Overflow
        } else if let Some(run) = row.workflow_run() {
            RailHitTarget::WorkflowRun {
                workflow_id: run.run.workflow_id.clone(),
                run_id: run.run.run_id.clone(),
            }
        } else if let Some(session) = row.session_id() {
            RailHitTarget::Session(session.to_string())
        } else if row.selectable() {
            RailHitTarget::Selectable
        } else {
            RailHitTarget::Inert
        };
        Self {
            target,
            #[cfg(test)]
            row: row.clone(),
            anchor,
            index,
        }
    }

    /// Whether the current rail projection still contains this rendered row.
    pub(in crate::ui::app) fn exists_in(
        &self,
        rows: &[super::super::rail::RailRow],
        lanes: &[crate::ui::agents::AgentLane],
    ) -> bool {
        self.anchor.as_ref().is_some_and(|anchor| {
            rows.iter()
                .any(|row| super::super::rail::rail_anchor(row, lanes).as_ref() == Some(anchor))
        })
    }

    /// Whether the cursor may land on this target.
    pub(in crate::ui::app) fn selectable(&self) -> bool {
        !matches!(&self.target, RailHitTarget::Inert)
    }
}
