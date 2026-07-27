//! Keyboard handling for the Workflows tab.
//!
//! Three panes share one keyboard, so which one has it decides what a key means
//! — the same settlement the Agents tab makes for its composer. `Tab` cycles the
//! panes and `Esc` steps back toward the rail; inside the copilot every
//! printable key types, so no instruction loses a character to a shortcut.
//!
//! The pane-independent actions (`r` to re-read the store, `Enter` to run) are
//! bound on the rail and the canvas only, for that reason.

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
        // Tab cycles the panes rather than the top-level tabs while this tab is
        // open. Shift+Tab still leaves, so a keyboard is never trapped here.
        if code == KeyCode::Tab && !shift {
            self.wf.focus = match self.wf.focus {
                WorkflowFocus::Rail => WorkflowFocus::Canvas,
                WorkflowFocus::Canvas => WorkflowFocus::Copilot,
                WorkflowFocus::Copilot => WorkflowFocus::Rail,
            };
            return WorkflowsKey::Handled(None);
        }
        match self.wf.focus {
            WorkflowFocus::Rail => self.workflow_rail_key(code),
            WorkflowFocus::Canvas => self.workflow_canvas_key(code),
            WorkflowFocus::Copilot => self.workflow_copilot_key(code, shift, alt),
        }
    }

    /// The catalogue rail: browse, run, simulate, re-read.
    fn workflow_rail_key(&mut self, code: KeyCode) -> WorkflowsKey {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_workflow_rail(true);
                WorkflowsKey::Handled(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_workflow_rail(false);
                WorkflowsKey::Handled(None)
            }
            // Right steps into the graph, which is where the cursor was heading
            // — the rail is a chooser and the canvas is the thing chosen.
            KeyCode::Right | KeyCode::Char('l') => {
                self.wf.focus = WorkflowFocus::Canvas;
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('r') => {
                self.reload_workflows();
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('d') => WorkflowsKey::Handled(self.dry_run_selected_workflow()),
            KeyCode::Enter => WorkflowsKey::Handled(self.run_selected_workflow()),
            _ => WorkflowsKey::Unhandled,
        }
    }

    /// The canvas: walk the graph, open the inspector, act on the workflow.
    fn workflow_canvas_key(&mut self, code: KeyCode) -> WorkflowsKey {
        match code {
            KeyCode::Left | KeyCode::Char('h') => {
                // Left off the first node steps back to the rail rather than
                // doing nothing, which is what makes the three panes feel like
                // one surface.
                if self
                    .workflow_layout()
                    .moved(self.wf.node_index, Move::Back)
                    .is_none()
                {
                    self.wf.focus = WorkflowFocus::Rail;
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
            KeyCode::Char('r') => {
                self.reload_workflows();
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char('d') => WorkflowsKey::Handled(self.dry_run_selected_workflow()),
            KeyCode::Enter => WorkflowsKey::Handled(self.run_selected_workflow()),
            KeyCode::Esc => {
                self.wf.focus = WorkflowFocus::Rail;
                WorkflowsKey::Handled(None)
            }
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
            KeyCode::PageUp => {
                self.wf.copilot_scroll = self.wf.copilot_scroll.saturating_add(5);
                WorkflowsKey::Handled(None)
            }
            KeyCode::PageDown => {
                self.wf.copilot_scroll = self.wf.copilot_scroll.saturating_sub(5);
                WorkflowsKey::Handled(None)
            }
            // Esc clears a draft first — that is the destructive-looking action
            // and must stay one keypress from a half-typed instruction. With
            // nothing to clear it steps back out to the rail.
            KeyCode::Esc => {
                if self.wf.draft.text.is_empty() {
                    self.wf.focus = WorkflowFocus::Rail;
                } else {
                    self.wf.draft = Draft::new();
                }
                WorkflowsKey::Handled(None)
            }
            KeyCode::Char(c) if !alt => {
                self.wf.draft =
                    insert_at(&self.wf.draft.text, self.wf.draft.cursor, &c.to_string());
                WorkflowsKey::Handled(None)
            }
            _ => WorkflowsKey::Unhandled,
        }
    }

    /// Run the selected workflow, refusing a disabled one.
    fn run_selected_workflow(&mut self) -> Option<Cmd> {
        let workflow = self.selected_workflow()?;
        if !workflow.enabled {
            let name = workflow.name.clone();
            self.set_status(format!("{name} is disabled"));
            return None;
        }
        let (id, name) = (workflow.id.clone(), workflow.name.clone());
        self.set_status(format!("Running {name}…"));
        Some(Cmd::RunWorkflow { id })
    }

    /// Simulate the selected workflow.
    ///
    /// Offered beside the run because it is the safe half of the same question:
    /// a dry run resolves every expression and starts no harness session, which
    /// is what an operator wants after an edit and before a real run.
    fn dry_run_selected_workflow(&mut self) -> Option<Cmd> {
        let workflow = self.selected_workflow()?;
        let (id, name) = (workflow.id.clone(), workflow.name.clone());
        self.set_status(format!("Simulating {name}…"));
        Some(Cmd::DryRunWorkflow { id })
    }
}
