//! Keyboard handling for the Routing tab and its fleet-management panes.

use crossterm::event::KeyCode;

use crate::ui::composer::{insert_at, Draft};
use crate::ui::multi_pane::{self, NavAction};
use medulla::daemon::pairing::REMOTE_JOIN_COMMAND;
use medulla::runtime::WorkerOp;

use super::super::types::{
    App, Cmd, Prompt, PromptKind, ROUTING_STRATEGIES, ROUTING_SUBPAGES, RP_ADD_HOST, RP_HARNESSES,
    RP_HOSTS, RP_STRATEGIES, RP_TEMPLATES, RP_WORKSPACES, SUBSCRIPTION_STRATEGIES,
};

impl App {
    /// Handle Routing navigation and the active pane's actions.
    pub(super) fn on_routing_key(&mut self, code: KeyCode) -> RoutingKey {
        match multi_pane::navigate(
            code,
            ROUTING_SUBPAGES.len(),
            &mut self.routing_index,
            &mut self.routing_focused,
            true,
        ) {
            NavAction::SelectionChanged | NavAction::Consumed => {
                return RoutingKey::Handled(None);
            }
            NavAction::Entered => {
                self.refresh_credential_status_if_needed();
                self.set_status(format!(
                    "{} · Esc to go back to the menu",
                    self.routing_subpage()
                ));
                // Capacity is declared, never streamed, so entering the page is
                // the moment to go and read it. Workspaces belongs here too: a
                // backend session starts with an empty `snapshot.capacity`, so
                // without this the page showed only this device's directories
                // until the operator happened to visit Harnesses or Templates
                // first — which reads as "the fleet has no workspaces".
                let cmd = matches!(
                    self.routing_index,
                    RP_HARNESSES | RP_TEMPLATES | RP_WORKSPACES
                )
                .then_some(Cmd::RefreshFleet);
                return RoutingKey::Handled(cmd);
            }
            NavAction::Left => {
                self.set_status("Routing · menu");
                return RoutingKey::Handled(None);
            }
            NavAction::Unhandled => {}
        }

        match self.routing_index {
            RP_HOSTS => self.hosts_key(code),
            RP_WORKSPACES => self.workspaces_key(code),
            RP_TEMPLATES => self.templates_key(code),
            RP_ADD_HOST => self.add_host_key(code),
            RP_HARNESSES => self.harnesses_key(code),
            RP_STRATEGIES => self.strategies_key(code),
            _ => RoutingKey::Unhandled,
        }
    }

    /// Browse and mutate the registered host roster.
    fn hosts_key(&mut self, code: KeyCode) -> RoutingKey {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.host_index = crate::ui::selection::moved(
                    self.host_index,
                    self.runtime.workers().len(),
                    true,
                );
                RoutingKey::Handled(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.host_index = crate::ui::selection::moved(
                    self.host_index,
                    self.runtime.workers().len(),
                    false,
                );
                RoutingKey::Handled(None)
            }
            KeyCode::Char('a') => {
                self.routing_index = RP_ADD_HOST;
                self.open_add_host_prompt();
                RoutingKey::Handled(None)
            }
            KeyCode::Char('s') | KeyCode::Enter => {
                let cmd = self.selected_host().map(|worker| {
                    self.set_status(format!(
                        "Selecting {}",
                        worker.label.as_deref().unwrap_or(&worker.address)
                    ));
                    Cmd::WorkerOp(WorkerOp::Select { id: worker.id })
                });
                RoutingKey::Handled(cmd)
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                let cmd = self.selected_host().map(|worker| {
                    self.set_status(format!(
                        "Removing {}",
                        worker.label.as_deref().unwrap_or(&worker.address)
                    ));
                    Cmd::WorkerOp(WorkerOp::Remove { id: worker.id })
                });
                RoutingKey::Handled(cmd)
            }
            KeyCode::Char('e') => {
                if let Some(worker) = self.selected_host() {
                    let mut draft = Draft::new();
                    if let Some(label) = &worker.label {
                        draft = insert_at("", 0, label);
                    }
                    self.prompt = Some(Prompt {
                        kind: PromptKind::HostEditLabel(worker.id),
                        title: format!("Edit label — {}", worker.address),
                        draft,
                    });
                    self.set_status("Edit label · Enter save · Esc cancel");
                }
                RoutingKey::Handled(None)
            }
            KeyCode::Char('r') => {
                let cmd = self.selected_host().map(|worker| {
                    self.set_status(format!("Refreshing {} details…", worker.id));
                    Cmd::WorkerOp(WorkerOp::RefreshDetails { id: worker.id })
                });
                RoutingKey::Handled(cmd)
            }
            _ => RoutingKey::Unhandled,
        }
    }

    /// Browse the agent-template catalog, open one, and re-read it on demand.
    fn templates_key(&mut self, code: KeyCode) -> RoutingKey {
        let rows = crate::ui::fleet::template_rows(&self.fleet_capacity(), &self.fleet_roster());
        // While the popup is open it owns scrolling and dismissal; everything
        // else still falls through to the catalog beneath it.
        if self.template_modal {
            match code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.template_modal = false;
                    self.template_scroll = 0;
                    return RoutingKey::Handled(None);
                }
                KeyCode::PageDown => {
                    self.template_scroll = self.template_scroll.saturating_add(5);
                    return RoutingKey::Handled(None);
                }
                KeyCode::PageUp => {
                    self.template_scroll = self.template_scroll.saturating_sub(5);
                    return RoutingKey::Handled(None);
                }
                _ => {}
            }
        }
        match code {
            KeyCode::Enter if !rows.is_empty() => {
                self.template_modal = true;
                self.template_scroll = 0;
                RoutingKey::Handled(None)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.template_index =
                    crate::ui::selection::moved(self.template_index, rows.len(), true);
                self.template_scroll = 0;
                RoutingKey::Handled(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.template_index =
                    crate::ui::selection::moved(self.template_index, rows.len(), false);
                self.template_scroll = 0;
                RoutingKey::Handled(None)
            }
            KeyCode::Char('r') => {
                self.set_status("Refreshing fleet…");
                RoutingKey::Handled(Some(Cmd::RefreshFleet))
            }
            _ => RoutingKey::Unhandled,
        }
    }

    /// Browse the workspace list and change this device's half of it.
    fn workspaces_key(&mut self, code: KeyCode) -> RoutingKey {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.workspace_index = crate::ui::selection::moved(
                    self.workspace_index,
                    self.workspace_rows().len(),
                    true,
                );
                RoutingKey::Handled(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.workspace_index = crate::ui::selection::moved(
                    self.workspace_index,
                    self.workspace_rows().len(),
                    false,
                );
                RoutingKey::Handled(None)
            }
            KeyCode::Char('a') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::WorkspaceAdd,
                    title: "Add workspace — absolute path to a directory".into(),
                    draft: Draft::new(),
                });
                self.set_status("Add workspace · Enter save · Esc cancel");
                RoutingKey::Handled(None)
            }
            KeyCode::Char('d') => {
                self.remove_selected_workspace();
                RoutingKey::Handled(None)
            }
            _ => RoutingKey::Unhandled,
        }
    }

    /// Open the existing host-address prompt from the dedicated Add Host pane,
    /// or copy the line the operator has to run on the machine being added.
    fn add_host_key(&mut self, code: KeyCode) -> RoutingKey {
        match code {
            KeyCode::Enter | KeyCode::Char('a') => {
                self.open_add_host_prompt();
                RoutingKey::Handled(None)
            }
            // Copying it here rather than retyping it there is the whole point:
            // this end is a local terminal, so the copy is free.
            KeyCode::Char('c') => {
                self.copy_line("the worker install line", REMOTE_JOIN_COMMAND);
                RoutingKey::Handled(None)
            }
            _ => RoutingKey::Unhandled,
        }
    }

    /// Build the host-add prompt shared by the list shortcut and Add Host page.
    fn open_add_host_prompt(&mut self) {
        self.prompt = Some(Prompt {
            kind: PromptKind::HostAdd,
            title: "Add host — address or @handle, optional label".into(),
            draft: Draft::new(),
        });
        self.set_status("Add host · Enter save · Esc cancel");
    }

    /// Re-read both halves of a harness on demand: the credentials it spends
    /// (detected locally) and the declarations it appears in (from the runtime).
    fn harnesses_key(&mut self, code: KeyCode) -> RoutingKey {
        if code == KeyCode::Char('r') {
            self.refresh_credential_status_if_needed();
            self.set_status("Harnesses refreshed");
            RoutingKey::Handled(Some(Cmd::RefreshFleet))
        } else {
            RoutingKey::Unhandled
        }
    }

    /// Browse and apply an automatic default-host strategy.
    fn strategies_key(&mut self, code: KeyCode) -> RoutingKey {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.subscription_strategy_focused {
                    self.subscription_strategy_index =
                        self.subscription_strategy_index.saturating_sub(1);
                } else {
                    self.routing_strategy_index = self.routing_strategy_index.saturating_sub(1);
                }
                RoutingKey::Handled(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.subscription_strategy_focused {
                    self.subscription_strategy_index = (self.subscription_strategy_index + 1)
                        .min(SUBSCRIPTION_STRATEGIES.len() - 1);
                } else {
                    self.routing_strategy_index =
                        (self.routing_strategy_index + 1).min(ROUTING_STRATEGIES.len() - 1);
                }
                RoutingKey::Handled(None)
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.subscription_strategy_focused = false;
                RoutingKey::Handled(None)
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                self.subscription_strategy_focused = true;
                RoutingKey::Handled(None)
            }
            KeyCode::Enter => {
                if self.subscription_strategy_focused {
                    let strategy =
                        SUBSCRIPTION_STRATEGIES[self.subscription_strategy_index].strategy;
                    self.persist_subscription_strategy_now(strategy);
                    RoutingKey::Handled(Some(Cmd::WorkerOp(WorkerOp::ApplySubscriptionStrategy {
                        strategy,
                    })))
                } else {
                    let strategy = ROUTING_STRATEGIES[self.routing_strategy_index].strategy;
                    // Persist the operator's selection (and set the status) before
                    // applying it to the runtime, so it reloads highlighted next start.
                    self.persist_routing_strategy_now(strategy);
                    RoutingKey::Handled(Some(Cmd::WorkerOp(WorkerOp::ApplyStrategy { strategy })))
                }
            }
            _ => RoutingKey::Unhandled,
        }
    }
}

mod types;
pub(super) use types::RoutingKey;
