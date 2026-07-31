//! Keyboard handling for the Workflows tab.
//!
//! Two levels of focus, split by *mode*, exactly as Settings and Routing split
//! theirs: the left-hand sidebar owns the keyboard until you step into the
//! content with `Enter` (or `→`), and `Esc` steps back out. The sidebar is the
//! catalogue rather than a fixed page list, so `↑↓` walk workflows and their
//! runs and the digits jump straight to one.
//!
//! `Tab` is deliberately *not* bound here. It is the one gesture for walking the
//! top-level views, and a tab that consumed it would be a tab inside a tab —
//! the copilot is reached with `c` instead, and `Esc` comes back.

use crossterm::event::KeyCode;

use medulla::ui::workflows::Move;

use crate::ui::composer::{delete_before, insert_at, Draft};

use super::super::types::{App, Cmd, WorkflowFocus};

/// Whether the Workflows tab consumed a key, and any command it produced.
pub(in super::super) enum WorkflowsKey {
    /// The tab handled the key.
    Handled(Option<Cmd>),
    /// A structural/global binding may handle it.
    Unhandled,
}

impl App {
    /// Handle a key for the Workflows tab.
    pub(super) fn on_workflows_key(
        &mut self,
        code: KeyCode,
        shift: bool,
        alt: bool,
    ) -> WorkflowsKey {
        match self.wf.focus {
            WorkflowFocus::Sidebar => self.workflow_sidebar_key(code),
            WorkflowFocus::Canvas => self.workflow_canvas_key(code),
            WorkflowFocus::Copilot => self.workflow_copilot_key(code, shift, alt),
        }
    }

    /// The catalogue sidebar: browse, jump, open, run, simulate, re-read.
    fn workflow_sidebar_key(&mut self, code: KeyCode) -> WorkflowsKey {
        // The digits jump to a workflow by position, as they jump to a subpage
        // everywhere else. Handled before the shared navigator because that one
        // indexes a fixed page list and this list is the store's.
        if let KeyCode::Char(digit @ '1'..='9') = code {
            let index = digit as usize - '1' as usize;
            if index < self.workflows.len() {
                self.select_workflow(index);
                self.wf.focus = WorkflowFocus::Canvas;
            }
            return WorkflowsKey::Handled(None);
        }
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_workflow_rail(true);
                WorkflowsKey::Handled(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_workflow_rail(false);
                WorkflowsKey::Handled(None)
            }
            // Enter and `→` both step into the content, which is what the rest
            // of the app's sidebars do. On the New row the content *is* the
            // copilot: there is no graph to open yet, and describing what you
            // want is the whole of making one.
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if self.wf.creating {
                    self.wf.focus = WorkflowFocus::Copilot;
                    self.set_status("Describe the workflow you want · Esc to go back");
                } else {
                    self.wf.focus = WorkflowFocus::Canvas;
                    self.set_status("Graph · Esc to go back to the list");
                }
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('c') => {
                self.wf.focus = WorkflowFocus::Copilot;
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('r') => {
                self.reload_workflows();
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('d') => WorkflowsKey::Handled(self.dry_run_selected_workflow()),
            KeyCode::Char('x') => WorkflowsKey::Handled(self.run_selected_workflow()),
            KeyCode::Char('u') => WorkflowsKey::Handled(self.undo_selected_workflow()),
            // `f` for "fix". Only meaningful with the cursor on a run, which is
            // where the operator is when they can see one failed.
            KeyCode::Char('f') => WorkflowsKey::Handled(self.repair_selected_run()),
            // `e` for "evolve": review this workflow and say what should change.
            // Distinct from `f`, which fixes one run now.
            KeyCode::Char('e') => WorkflowsKey::Handled(self.evolve_selected_workflow()),
            // The operator's half of a review. Deliberately keys an agent has no
            // equivalent of: applying a proposed change is a person's decision.
            KeyCode::Char('a') => WorkflowsKey::Handled(self.accept_selected_proposal()),
            KeyCode::Char('n') => WorkflowsKey::Handled(self.reject_selected_proposal()),
            // Every other character is swallowed so a stray letter cannot fire a
            // content-pane action from the menu — the settlement Settings makes.
            KeyCode::Char(_) => WorkflowsKey::Handled(None),
            _ => WorkflowsKey::Unhandled,
        }
    }

    /// The canvas: walk the graph, open the inspector, act on the workflow.
    fn workflow_canvas_key(&mut self, code: KeyCode) -> WorkflowsKey {
        match code {
            // Esc unwinds one level at a time, the same gesture every other
            // content pane in the app answers to: the inspector closes back to
            // the graph it is about, and the graph steps back out to the list.
            KeyCode::Esc => {
                if self.wf.inspector_open {
                    self.wf.inspector_open = false;
                    self.set_status("Graph · Esc to go back to the list");
                } else {
                    self.wf.focus = WorkflowFocus::Sidebar;
                    self.set_status("Workflows · list");
                }
                WorkflowsKey::Handled(None)
            }
            KeyCode::Left | KeyCode::Char('h') => {
                // Left off the first node steps back to the list rather than
                // doing nothing, which is what makes the panes feel like one
                // surface.
                if self
                    .workflow_layout()
                    .moved(self.wf.node_index, Move::Back)
                    .is_none()
                {
                    self.wf.focus = WorkflowFocus::Sidebar;
                } else {
                    self.move_graph_cursor(Move::Back);
                }
                WorkflowsKey::Handled(None)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.move_graph_cursor(Move::Forward);
                WorkflowsKey::Handled(None)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_graph_cursor(Move::LaneUp);
                WorkflowsKey::Handled(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_graph_cursor(Move::LaneDown);
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('i') => {
                self.wf.inspector_open = !self.wf.inspector_open;
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('c') => {
                self.wf.focus = WorkflowFocus::Copilot;
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('r') => {
                self.reload_workflows();
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('d') => WorkflowsKey::Handled(self.dry_run_selected_workflow()),
            KeyCode::Char('x') => WorkflowsKey::Handled(self.run_selected_workflow()),
            KeyCode::Char('u') => WorkflowsKey::Handled(self.undo_selected_workflow()),
            KeyCode::Char('f') => WorkflowsKey::Handled(self.repair_selected_run()),
            KeyCode::PageUp => {
                self.wf.preview_scroll = self.wf.preview_scroll.saturating_sub(6);
                WorkflowsKey::Handled(None)
            }
            KeyCode::PageDown => {
                self.wf.preview_scroll = self.wf.preview_scroll.saturating_add(6);
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('a') => WorkflowsKey::Handled(self.accept_selected_proposal()),
            KeyCode::Char('n') => WorkflowsKey::Handled(self.reject_selected_proposal()),
            _ => WorkflowsKey::Unhandled,
        }
    }

    /// The copilot composer: every printable key types.
    fn workflow_copilot_key(&mut self, code: KeyCode, shift: bool, alt: bool) -> WorkflowsKey {
        match code {
            KeyCode::Enter if shift || alt => {
                self.wf.draft = insert_at(&self.wf.draft.text, self.wf.draft.cursor, "\n");
                WorkflowsKey::Handled(None)
            }
            KeyCode::Enter => WorkflowsKey::Handled(self.submit_copilot()),
            KeyCode::Backspace | KeyCode::Delete => {
                self.wf.draft = delete_before(&self.wf.draft.text, self.wf.draft.cursor);
                WorkflowsKey::Handled(None)
            }
            KeyCode::Left => {
                self.wf.draft.cursor = self.wf.draft.cursor.saturating_sub(1);
                WorkflowsKey::Handled(None)
            }
            KeyCode::Right => {
                self.wf.draft.cursor =
                    (self.wf.draft.cursor + 1).min(self.wf.draft.text.chars().count());
                WorkflowsKey::Handled(None)
            }
            // A page, not a fixed five rows. The render clamps the offset to
            // what the content can actually scroll by and writes it back, so
            // holding PageUp settles at the top rather than banking presses
            // PageDown then has to spend before anything moves.
            KeyCode::PageUp => {
                self.wf.copilot_scroll = self.wf.copilot_scroll.saturating_add(self.copilot_page());
                WorkflowsKey::Handled(None)
            }
            KeyCode::PageDown => {
                self.wf.copilot_scroll = self.wf.copilot_scroll.saturating_sub(self.copilot_page());
                WorkflowsKey::Handled(None)
            }
            // Esc clears a draft first — that is the destructive-looking action
            // and must stay one keypress from a half-typed instruction. With
            // nothing to clear it steps back to the graph it is about.
            KeyCode::Esc => {
                if !self.wf.draft.text.is_empty() {
                    self.wf.draft = Draft::new();
                } else if self.wf.creating {
                    // A workflow that does not exist has no graph to step back
                    // to, so the list is the level below.
                    self.wf.focus = WorkflowFocus::Sidebar;
                } else {
                    self.wf.focus = WorkflowFocus::Canvas;
                }
                WorkflowsKey::Handled(None)
            }
            // Retry, but only with nothing typed: every printable key in this
            // pane types, and an `r` that sometimes did something else instead
            // would be worse than no retry at all.
            KeyCode::Char('r') if self.wf.draft.text.is_empty() => {
                WorkflowsKey::Handled(self.retry_copilot())
            }
            KeyCode::Char(c) if !alt => {
                self.wf.draft =
                    insert_at(&self.wf.draft.text, self.wf.draft.cursor, &c.to_string());
                WorkflowsKey::Handled(None)
            }
            _ => WorkflowsKey::Unhandled,
        }
    }

    /// Take back the last edit to the selected workflow.
    ///
    /// Deliberately not confirmed. Undo is the *recovery* gesture — the one an
    /// operator reaches for after the copilot did something they did not want —
    /// and a confirmation prompt in front of it would put a decision between
    /// them and the fix. It is also itself undoable: the restore is snapshotted
    /// like any other write, so pressing `u` twice returns to where you were.
    fn undo_selected_workflow(&mut self) -> Option<Cmd> {
        if self.wf.creating {
            self.set_status("Nothing to undo — this workflow has not been created yet");
            return None;
        }
        let id = self.selected_workflow()?.id.clone();
        Some(Cmd::UndoWorkflow { id })
    }

    /// Run the selected workflow, refusing a disabled one.
    fn run_selected_workflow(&mut self) -> Option<Cmd> {
        if self.wf.creating {
            self.set_status("Nothing to run yet — describe the workflow first");
            return None;
        }
        let workflow = self.selected_workflow()?;
        if !workflow.enabled {
            let name = workflow.name.clone();
            self.set_status(format!("{name} is disabled"));
            return None;
        }
        let (id, name) = (workflow.id.clone(), workflow.name.clone());
        let declared = workflow.inputs.clone();
        if !declared.is_empty() {
            return self.begin_workflow_inputs(id, false, declared);
        }
        self.set_status(format!("Running {name}…"));
        Some(Cmd::RunWorkflow {
            id,
            inputs: Default::default(),
        })
    }

    /// Open the first of one prompt per declared input, ahead of a run.
    ///
    /// Returns `None` because the run is not dispatched yet: the command is
    /// emitted by [`super::super::commands`] when the last field is submitted.
    /// Cancelling any prompt abandons the whole set — the collected values live
    /// on the prompt, so there is nothing left behind to leak into a later run.
    fn begin_workflow_inputs(
        &mut self,
        workflow_id: String,
        dry_run: bool,
        remaining: Vec<medulla::workflows::WorkflowInput>,
    ) -> Option<Cmd> {
        self.open_workflow_input_prompt(workflow_id, dry_run, remaining, Default::default());
        None
    }

    /// Simulate the selected workflow.
    ///
    /// Offered beside the run because it is the safe half of the same question:
    /// a dry run resolves every expression and starts no harness session, which
    /// is what an operator wants after an edit and before a real run.
    fn dry_run_selected_workflow(&mut self) -> Option<Cmd> {
        if self.wf.creating {
            self.set_status("Nothing to simulate yet — describe the workflow first");
            return None;
        }
        let workflow = self.selected_workflow()?;
        let (id, name) = (workflow.id.clone(), workflow.name.clone());
        let declared = workflow.inputs.clone();
        if !declared.is_empty() {
            return self.begin_workflow_inputs(id, true, declared);
        }
        self.set_status(format!("Simulating {name}…"));
        Some(Cmd::DryRunWorkflow {
            id,
            inputs: Default::default(),
        })
    }
}

#[cfg(test)]
#[path = "workflows_tests.rs"]
mod tests;
