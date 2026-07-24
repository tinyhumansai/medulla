//! Keyboard handling for the Routing tab and its worker-management panes.

use crossterm::event::KeyCode;

use crate::ui::composer::{insert_at, Draft};
use medulla::runtime::WorkerOp;

use super::super::types::{
    App, Cmd, Prompt, PromptKind, ROUTING_SUBPAGES, RP_ADD_WORKER, RP_WORKERS,
};

impl App {
    /// Handle Routing navigation and the active pane's actions.
    pub(super) fn on_routing_key(&mut self, code: KeyCode) -> RoutingKey {
        if let KeyCode::Char(d @ '1'..='4') = code {
            self.routing_index = d as usize - '1' as usize;
            self.routing_focused = true;
            return RoutingKey::Handled(None);
        }

        if !self.routing_focused {
            return match code {
                KeyCode::Up => {
                    self.routing_index = self.routing_index.saturating_sub(1);
                    RoutingKey::Handled(None)
                }
                KeyCode::Down => {
                    self.routing_index = (self.routing_index + 1).min(ROUTING_SUBPAGES.len() - 1);
                    RoutingKey::Handled(None)
                }
                KeyCode::Enter => {
                    self.routing_focused = true;
                    RoutingKey::Handled(None)
                }
                KeyCode::Char(_) => RoutingKey::Handled(None),
                _ => RoutingKey::Unhandled,
            };
        }

        if code == KeyCode::Esc {
            self.routing_focused = false;
            self.set_status("Routing · menu");
            return RoutingKey::Handled(None);
        }

        match self.routing_index {
            RP_WORKERS => self.workers_key(code),
            RP_ADD_WORKER => self.add_worker_key(code),
            _ => RoutingKey::Handled(None),
        }
    }

    /// Browse and mutate the registered worker roster.
    fn workers_key(&mut self, code: KeyCode) -> RoutingKey {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.worker_index = self.worker_index.saturating_sub(1);
                RoutingKey::Handled(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = self.runtime.workers().len().saturating_sub(1);
                self.worker_index = (self.worker_index + 1).min(max);
                RoutingKey::Handled(None)
            }
            KeyCode::Char('a') => {
                self.routing_index = RP_ADD_WORKER;
                self.open_add_worker_prompt();
                RoutingKey::Handled(None)
            }
            KeyCode::Char('s') | KeyCode::Enter => {
                let cmd = self.selected_worker().map(|worker| {
                    self.set_status(format!(
                        "Selecting {}",
                        worker.label.as_deref().unwrap_or(&worker.address)
                    ));
                    Cmd::WorkerOp(WorkerOp::Select { id: worker.id })
                });
                RoutingKey::Handled(cmd)
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                let cmd = self.selected_worker().map(|worker| {
                    self.set_status(format!(
                        "Removing {}",
                        worker.label.as_deref().unwrap_or(&worker.address)
                    ));
                    Cmd::WorkerOp(WorkerOp::Remove { id: worker.id })
                });
                RoutingKey::Handled(cmd)
            }
            KeyCode::Char('e') => {
                if let Some(worker) = self.selected_worker() {
                    let mut draft = Draft::new();
                    if let Some(label) = &worker.label {
                        draft = insert_at("", 0, label);
                    }
                    self.prompt = Some(Prompt {
                        kind: PromptKind::WorkerEditLabel(worker.id),
                        title: format!("Edit label — {}", worker.address),
                        draft,
                    });
                    self.set_status("Edit label · Enter save · Esc cancel");
                }
                RoutingKey::Handled(None)
            }
            _ => RoutingKey::Handled(None),
        }
    }

    /// Open the existing worker-address prompt from the dedicated Add Worker pane.
    fn add_worker_key(&mut self, code: KeyCode) -> RoutingKey {
        if matches!(code, KeyCode::Enter | KeyCode::Char('a')) {
            self.open_add_worker_prompt();
        }
        RoutingKey::Handled(None)
    }

    /// Build the worker-add prompt shared by the list shortcut and Add Worker page.
    fn open_add_worker_prompt(&mut self) {
        self.prompt = Some(Prompt {
            kind: PromptKind::WorkerAdd,
            title: "Add worker — address or @handle, optional label".into(),
            draft: Draft::new(),
        });
        self.set_status("Add worker · Enter save · Esc cancel");
    }
}

/// Whether Routing consumed a key and its optional runtime command.
pub(super) enum RoutingKey {
    /// Routing handled the key.
    Handled(Option<Cmd>),
    /// A structural/global binding may handle the key.
    Unhandled,
}
