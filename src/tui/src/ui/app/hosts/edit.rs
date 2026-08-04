//! Editing an agent from the Hosts page: role assignment, undeclaring, and
//! renaming — each written to `[fleet].agentDeclarations` rather than to the
//! live roster alone.
//!
//! The distinction this file exists for: [`HubHandle::set_roles`] moves the
//! roles on the roster this process is holding, and the roster is rebuilt from
//! the declarations at every launch. Assigning a role that way is a change the
//! operator watches take effect and then loses. So every edit here writes the
//! declaration *first* and mutates the live roster second — and when the write
//! fails, the live change is not made either, because a UI showing a role the
//! file does not have is worse than one that refused.
//!
//! **The no-config-file path.** An install with nowhere to write — no
//! `--config`, no discovered file — still has a live roster in front of the
//! operator, so an edit is not refused there. It applies to *this run*, in both
//! places at once (the in-memory declaration list and the roster), and the
//! status says how long it lasts. That is one rule for all three edits here, and
//! it is the same one `save_workspaces` follows for `[host].workspaces`.
//!
//! It is deliberately not the same thing as a write *failure*. A failed write
//! means the file exists and disagrees, so nothing is applied at all — see
//! [`persist_agent_roles`](App::persist_agent_roles). Having no file to
//! disagree with is not a failure, and refusing there would leave the operator
//! unable to touch a roster they can see. What is never allowed is the third
//! option: applying the edit and saying nothing, which reads as "saved".
//!
//! [`HubHandle::set_roles`]: medulla::hub::HubHandle::set_roles

use medulla::runtime::{AgentDeclaration, WorkerOp};
use medulla::ui::hosts::{HostAgentRow, HostRow};

use super::super::types::{App, Cmd};

impl App {
    /// Toggle `role` on the agent under the cursor, persisting the result.
    ///
    /// Returns the roster mutation to apply on top of the write, so the
    /// orchestrator starts (or stops) routing that role here without waiting for
    /// a restart. `None` when nothing was changed, when the agent is not in the
    /// live roster, or when the write failed — in every one of those cases the
    /// status line says why.
    pub(in crate::ui::app) fn toggle_selected_agent_role(&mut self, role: &str) -> Option<Cmd> {
        let host = self.selected_host_row()?;
        let agent = self.selected_host_agent()?;
        if !agent.editable {
            self.set_status(format!(
                "{} is declared on {} — assign its roles there",
                agent.agent_id, host.label
            ));
            return None;
        }
        let mut roles = agent.roles.clone();
        let assigned = match roles.iter().position(|held| held == role) {
            Some(at) => {
                roles.remove(at);
                false
            }
            None => {
                roles.push(role.to_string());
                true
            }
        };
        let outcome = if assigned {
            format!("{} now offered for {role}", agent.agent_id)
        } else {
            format!("{} no longer offered for {role}", agent.agent_id)
        };
        if !self.persist_agent_roles(&host, &agent, roles.clone(), outcome) {
            return None;
        }
        // Only a live agent has a roster entry to move. A declared one that is
        // not running has nothing to re-register; its roles ride the declaration
        // into the roster the next time it starts.
        agent.live.then(|| {
            Cmd::WorkerOp(WorkerOp::SetRoles {
                id: agent.agent_id.clone(),
                roles,
            })
        })
    }

    /// Write `roles` onto the agent's declaration, narrating either outcome.
    ///
    /// Returns whether the caller may go on to move the live roster. An agent
    /// the roster knows but no declaration covers — the migration seed — is
    /// *declared here*, from what the roster reports, because a role assigned to
    /// something nobody wrote down has nowhere to persist to.
    fn persist_agent_roles(
        &mut self,
        host: &HostRow,
        agent: &HostAgentRow,
        roles: Vec<String>,
        outcome: String,
    ) -> bool {
        let current = self.loaded.config.fleet.agent_declarations.clone();
        let mut declaration = match medulla::config::agent_declaration(&current, &agent.agent_id) {
            Some(held) => held.clone(),
            None => {
                let Some(harness) = agent.harness.clone().filter(|h| !h.trim().is_empty()) else {
                    // Nothing to write down faithfully: a declaration invents an
                    // agent, and inventing one with no harness would advertise a
                    // placement that cannot run.
                    self.set_status(format!(
                        "{} reports no harness type, so its roles cannot be saved",
                        agent.agent_id
                    ));
                    return false;
                };
                let Some(workspace) = agent.workspace.clone().filter(|w| !w.trim().is_empty())
                else {
                    // The same reason as the harness, one step further on: an
                    // agent is `harness × workspace`, and a declaration with no
                    // directory is one `open_new_session` then refuses with
                    // "declares no workspace". Saving the role would leave the
                    // operator looking at an agent they cannot use.
                    self.set_status(format!(
                        "{} reports no workspace, so its roles cannot be saved",
                        agent.agent_id
                    ));
                    return false;
                };
                AgentDeclaration::new(agent.agent_id.clone(), host.id.clone(), harness, workspace)
            }
        };
        declaration.roles = roles;
        let Some(path) = self.config_path.clone() else {
            // Nowhere to write, so the edit is this run's. It has to land on the
            // declaration list as well as the roster: the Hosts tree reads a
            // declared agent's roles from the declaration, so updating only the
            // roster would redraw the row with the roles it had before while
            // `SetRoles` carried the new ones.
            medulla::config::upsert_agent_declaration(
                &mut self.loaded.config.fleet.agent_declarations,
                declaration,
            );
            self.set_status(format!("{outcome} (this run only — no config file)"));
            return true;
        };
        match medulla::config::declare_agent(&path, &current, declaration) {
            Ok(declarations) => {
                self.loaded.config.fleet.agent_declarations = declarations;
                self.set_status(outcome);
                true
            }
            Err(error) => {
                self.set_status(format!("Roles were not saved: {error}"));
                false
            }
        }
    }

    /// Undeclare the agent under the cursor, if this machine declared it.
    ///
    /// Removing only undeclares: the workspace directory is left alone, because
    /// the operator asked the orchestrator to stop placing work there, not to
    /// lose their checkout. Returns whether the declaration went, so the caller
    /// can decide what to do about the live roster entry.
    pub(in crate::ui::app) fn undeclare_selected_agent(&mut self) -> bool {
        let Some(agent) = self.selected_host_agent() else {
            return false;
        };
        if !agent.declared || !agent.editable {
            return false;
        }
        let Some(path) = self.config_path.clone() else {
            // Same rule as a role edit: nowhere to write is not a refusal, it is
            // an edit that lasts one run — and the status is what keeps the
            // agent's return at the next launch from being a surprise.
            medulla::config::remove_agent_declaration(
                &mut self.loaded.config.fleet.agent_declarations,
                &agent.agent_id,
            );
            self.set_status(format!(
                "Undeclared {} (this run only — no config file)",
                agent.agent_id
            ));
            return true;
        };
        let current = self.loaded.config.fleet.agent_declarations.clone();
        match medulla::config::undeclare_agent(&path, &current, &agent.agent_id) {
            Ok(declarations) => {
                self.loaded.config.fleet.agent_declarations = declarations;
                self.set_status(format!(
                    "Undeclared {} · its files are untouched",
                    agent.agent_id
                ));
                true
            }
            Err(error) => {
                self.set_status(format!("{} was not undeclared: {error}", agent.agent_id));
                false
            }
        }
    }

    /// Persist a renamed agent, when the label just edited belongs to one this
    /// machine declares.
    ///
    /// The roster label is this run's; the declaration's `name` is the one that
    /// comes back. A blank name clears it, which returns the agent to being
    /// named by the renderer rather than by a label nobody typed.
    pub(in crate::ui::app) fn persist_agent_name(&mut self, agent_id: &str, name: &str) {
        let current = self.loaded.config.fleet.agent_declarations.clone();
        let Some(declaration) = medulla::config::agent_declaration(&current, agent_id) else {
            return;
        };
        let mut declaration = declaration.clone();
        declaration.name = (!name.trim().is_empty()).then(|| name.trim().to_string());
        let Some(path) = self.config_path.clone() else {
            // The roster label has already changed by the time this runs, so a
            // silent return is the one outcome the module forbids: the operator
            // would read the new name off the row and have nothing telling them
            // it goes away at the next launch.
            medulla::config::upsert_agent_declaration(
                &mut self.loaded.config.fleet.agent_declarations,
                declaration,
            );
            self.set_status(format!(
                "Renamed {agent_id} (this run only — no config file)"
            ));
            return;
        };
        match medulla::config::declare_agent(&path, &current, declaration) {
            Ok(declarations) => self.loaded.config.fleet.agent_declarations = declarations,
            Err(error) => self.set_status(format!("The new name was not saved: {error}")),
        }
    }
}
