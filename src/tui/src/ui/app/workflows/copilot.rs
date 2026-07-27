//! The copilot thread beside the graph.
//!
//! One conversation per workflow, held in the app rather than the SDK because it
//! is screen state: what the operator has asked *this session*, and whether a
//! turn is in flight. The turn itself runs off-thread
//! ([`medulla::workflows::CopilotSession`]) and reports back through the event
//! loop, so everything here is bookkeeping around that.

use medulla::ui::workflows::CopilotState;

use super::super::types::{App, Cmd};

/// The key the not-yet-a-workflow thread is filed under.
///
/// A workflow that does not exist has no id to key its conversation by, but the
/// thread still has to survive the operator walking away from the New row and
/// coming back. The leading control character cannot occur in an id: those come
/// from file stems, and the store never yields one containing `\u{1}`.
pub(in crate::ui::app) const NEW_THREAD: &str = "\u{1}new-workflow";

impl App {
    /// The copilot thread the rail cursor is on, creating it on first use.
    ///
    /// The New row has a thread of its own, filed under [`NEW_THREAD`] — the
    /// conversation that will produce a workflow, held before there is one to
    /// name it after.
    ///
    /// Returns `None` only when a workflow is selected and the catalogue cannot
    /// produce it.
    pub(in crate::ui::app) fn copilot_mut(&mut self) -> Option<&mut CopilotState> {
        let id = self.copilot_key()?;
        Some(
            self.wf
                .copilots
                .entry(id.clone())
                .or_insert_with(|| CopilotState::new(id)),
        )
    }

    /// The thread the rail cursor is on, if it has one yet.
    pub(in crate::ui::app) fn copilot(&self) -> Option<&CopilotState> {
        self.wf.copilots.get(&self.copilot_key()?)
    }

    /// Which thread the rail cursor addresses.
    fn copilot_key(&self) -> Option<String> {
        if self.wf.creating {
            return Some(NEW_THREAD.to_string());
        }
        Some(self.selected_workflow()?.id.clone())
    }

    /// Whether the selected workflow has a turn in flight.
    pub(in crate::ui::app) fn copilot_busy(&self) -> bool {
        self.copilot().is_some_and(|thread| thread.busy)
    }

    /// Rows one `PageUp`/`PageDown` moves the copilot transcript by.
    ///
    /// A page less one row, so the line the operator was reading stays on
    /// screen and gives them their place back. Derived from the last drawn area
    /// rather than a constant: the pane's height depends on the terminal, and a
    /// fixed step pages twice on a tall one and overshoots on a short one.
    pub(in crate::ui::app) fn copilot_page(&self) -> usize {
        // The header, tab strip, footer, and status row above the tab, then the
        // pane's own borders, its hint row, and the smallest composer. An
        // estimate rather than the measured rect, because the transcript's
        // height is only known inside a draw pass — and being a row out only
        // changes how far one keypress travels, which the render then clamps.
        const CHROME: usize = 11;
        const MIN_STEP: usize = 1;
        (self.area.height as usize)
            .saturating_sub(CHROME)
            .max(MIN_STEP)
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
        // The New row's turn creates rather than edits, so it is a different
        // command — but the same draft, the same thread bookkeeping, and the
        // same refusal while one is in flight.
        let creating = self.wf.creating;
        let workflow = if creating {
            NEW_THREAD.to_string()
        } else {
            self.selected_workflow()?.id.clone()
        };
        self.wf.draft = crate::ui::composer::Draft::new();
        self.wf.copilot_scroll = 0;
        self.copilot_mut()?.ask(&instruction);
        if creating {
            self.set_status("Copilot · building a new workflow…");
            return Some(Cmd::CreateWorkflow {
                thread: workflow,
                instruction,
            });
        }
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
    ///
    /// Handed to [`CopilotState::progress`] rather than `status` so a frame
    /// announcing a tool call becomes a tool line — the same `⏺` the
    /// orchestrator's transcript draws — instead of dim chatter that ages out.
    pub fn copilot_status(&mut self, workflow: &str, line: String) {
        if let Some(thread) = self.wf.copilots.get_mut(workflow) {
            thread.progress(&line);
        }
    }

    /// Record a finished copilot turn: its reply, what it changed, and any
    /// workflow it brought into existence.
    ///
    /// Re-reads the catalogue and the graph when something changed, so the pane
    /// beside the transcript shows the edit the transcript just described.
    pub fn copilot_finished(
        &mut self,
        workflow: &str,
        reply: String,
        changes: Vec<String>,
        created: Option<String>,
    ) {
        let changed = !changes.is_empty();
        if let Some(thread) = self.wf.copilots.get_mut(workflow) {
            thread.changed(changes);
            thread.reply(reply);
        }
        if !changed {
            return;
        }
        self.reload_workflows();
        match created {
            Some(id) => self.adopt_new_workflow(&id),
            None => self.set_status(format!("{workflow} updated")),
        }
    }

    /// Select the workflow a create turn just made, and give it the thread that
    /// made it.
    ///
    /// The conversation moves rather than being discarded: it is the record of
    /// why this workflow looks the way it does, and the operator's next
    /// instruction is almost always a follow-up to it. The New row is left with
    /// a clean thread for the next workflow.
    fn adopt_new_workflow(&mut self, id: &str) {
        let Some(index) = self
            .workflow_summaries()
            .iter()
            .position(|summary| summary.id == id)
        else {
            // The store says it is not there. Reported rather than silently
            // ignored: the agent believed it created something.
            self.set_status(format!("Created {id}, but it is not in the catalogue"));
            return;
        };
        if let Some(mut thread) = self.wf.copilots.remove(NEW_THREAD) {
            thread.workflow_id = id.to_string();
            self.wf.copilots.insert(id.to_string(), thread);
        }
        // Clears `creating`, so the rail cursor lands on the new workflow and
        // the content pane draws its graph.
        self.select_workflow(index);
        self.set_status(format!("Created {id}"));
    }

    /// Record a copilot turn that failed.
    pub fn copilot_failed(&mut self, workflow: &str, error: String) {
        if let Some(thread) = self.wf.copilots.get_mut(workflow) {
            thread.failed(error.clone());
        }
        self.set_status(format!("Copilot failed: {error}"));
    }
}
