//! The copilot thread beside the graph.
//!
//! One conversation per workflow, held in the app rather than the SDK because it
//! is screen state: what the operator has asked *this session*, and whether a
//! turn is in flight. The turn itself runs off-thread
//! ([`medulla::workflows::CopilotSession`]) and reports back through the event
//! loop, so everything here is bookkeeping around that.

use medulla::ui::workflows::CopilotState;

use super::super::types::{App, Cmd};

impl App {
    /// The copilot thread for the selected workflow, creating it on first use.
    ///
    /// Returns `None` when nothing is selected — an empty catalogue has no
    /// conversation to have.
    pub(in crate::ui::app) fn copilot_mut(&mut self) -> Option<&mut CopilotState> {
        let id = self.selected_workflow()?.id.clone();
        Some(
            self.wf
                .copilots
                .entry(id.clone())
                .or_insert_with(|| CopilotState::new(id)),
        )
    }

    /// The selected workflow's copilot thread, if it has one yet.
    pub(in crate::ui::app) fn copilot(&self) -> Option<&CopilotState> {
        let id = &self.selected_workflow()?.id;
        self.wf.copilots.get(id)
    }

    /// Whether the selected workflow has a turn in flight.
    pub(in crate::ui::app) fn copilot_busy(&self) -> bool {
        self.copilot().is_some_and(|thread| thread.busy)
    }

    /// Send the composer's draft to the copilot.
    ///
    /// Refuses while a turn is in flight rather than queueing: the agent is
    /// editing the graph, and a second instruction written against the pre-edit
    /// graph is one the operator did not mean to give.
    pub(in crate::ui::app) fn submit_copilot(&mut self) -> Option<Cmd> {
        let instruction = self.wf.draft.text.trim().to_string();
        if instruction.is_empty() {
            return None;
        }
        if self.copilot_busy() {
            self.set_status("The copilot is still working — wait for it to finish");
            return None;
        }
        let workflow = self.selected_workflow()?.id.clone();
        self.wf.draft = crate::ui::composer::Draft::new();
        self.wf.copilot_scroll = 0;
        self.copilot_mut()?.ask(&instruction);
        self.set_status(format!("Copilot · {workflow}"));
        Some(Cmd::CopilotTurn {
            workflow,
            instruction,
        })
    }

    /// Record a progress line from a running copilot turn.
    ///
    /// Addressed by workflow id rather than applied to the selection: the
    /// operator may well have moved the rail on while the turn runs, and the
    /// line belongs to the thread that asked for it.
    pub fn copilot_status(&mut self, workflow: &str, line: String) {
        if let Some(thread) = self.wf.copilots.get_mut(workflow) {
            thread.status(line);
        }
    }

    /// Record a finished copilot turn: its reply, and what it changed.
    ///
    /// Re-reads the catalogue and the graph when something changed, so the pane
    /// beside the transcript shows the edit the transcript just described.
    pub fn copilot_finished(&mut self, workflow: &str, reply: String, changes: Vec<String>) {
        let changed = !changes.is_empty();
        if let Some(thread) = self.wf.copilots.get_mut(workflow) {
            thread.changed(changes);
            thread.reply(reply);
        }
        if changed {
            self.reload_workflows();
            self.set_status(format!("{workflow} updated"));
        }
    }

    /// Record a copilot turn that failed.
    pub fn copilot_failed(&mut self, workflow: &str, error: String) {
        if let Some(thread) = self.wf.copilots.get_mut(workflow) {
            thread.failed(error.clone());
        }
        self.set_status(format!("Copilot failed: {error}"));
    }
}
