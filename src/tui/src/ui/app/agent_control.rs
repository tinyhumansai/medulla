//! Declaring agents, and opening sessions under them.
//!
//! The two writes the Agents tab makes. **Declaring** is `harness × workspace`
//! written to `[fleet].agentDeclarations` — it starts nothing, and the agent it
//! produces has a rail row from that moment whether or not anything ever runs in
//! it. **Opening a session** is the opposite: it starts a process and writes
//! nothing, inheriting the harness and the directory from the declaration rather
//! than asking again.
//!
//! Both reuse the picker in [`harness_control`](super::harness_control): picking
//! a CLI and a directory is the same two steps either way, and the intent it
//! carries ([`PickerPurpose`]) is what decides which of these two ends it lands
//! in.
//!
//! Persistence goes through [`declare_agent`](medulla::config::declare_agent),
//! which writes the file and hands back the list *as written*. That list is
//! assigned straight into the loaded config, so a failed write leaves the rail
//! showing exactly what is on disk rather than an agent that will not survive a
//! restart.

use medulla::config::{declare_agent, declared_agent_ids};
use medulla::runtime::{suggest_agent_id, AgentDeclaration, WorkspaceRef, WorkspaceStrategy};

use crate::ui::harness_pane::HarnessChoice;

use super::types::{
    tab_pos, App, HarnessPicker, HarnessPickerStep, PickerPurpose, Prompt, PromptKind,
};

impl App {
    /// Open the create-agent flow: harness type, then workspace, then a name.
    ///
    /// Refuses on a device that is not hosting rather than opening a picker with
    /// nothing in it: an agent is declared *on a host*, and there is none here to
    /// declare it on.
    pub(in crate::ui::app) fn open_new_agent_picker(&mut self) {
        let Some(harnesses) = self.harnesses.clone() else {
            self.set_status("This device is not hosting, so it has no agents to declare");
            return;
        };
        let choices = harnesses.choices();
        if choices.is_empty() {
            self.set_status("No harness CLIs found on this device");
            return;
        }
        self.harness_picker = Some(HarnessPicker {
            purpose: PickerPurpose::DeclareAgent,
            choices,
            index: 0,
            step: HarnessPickerStep::Harness,
            cwd: harnesses.workspace.clone(),
            workspace_query: String::new(),
            workspace_choices: Vec::new(),
            workspace_index: 0,
            workspace_picked: false,
            managed: true,
        });
        self.set_status("New agent · pick a harness · Enter workspace · Esc cancel");
    }

    /// Ask what to call the agent about to be declared for this pair.
    ///
    /// A single-line prompt rather than a third picker step: the answer is
    /// optional, and everything the operator needs to decide it — the CLI and the
    /// directory — is in the question.
    pub(in crate::ui::app) fn prompt_agent_name(&mut self, harness: &str, workspace: &str) {
        let folder = std::path::Path::new(workspace)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| workspace.to_string());
        self.prompt = Some(Prompt {
            title: format!("Name this agent — {harness} × {folder} (blank keeps the default)"),
            draft: Default::default(),
            kind: PromptKind::AgentName {
                harness: harness.to_string(),
                workspace: workspace.to_string(),
            },
        });
    }

    /// Declare `harness × workspace` on this host and adopt the written list.
    ///
    /// The id is minted rather than typed: it is the dispatch target, and a
    /// collision would silently route work to the wrong agent, so
    /// [`suggest_agent_id`] is given every id already declared *anywhere* in the
    /// fleet — not only this host's.
    pub(in crate::ui::app) fn declare_new_agent(
        &mut self,
        harness: &str,
        workspace: &str,
        name: &str,
    ) {
        let Some(path) = self.config_path.clone() else {
            self.set_status("No config file to declare an agent in");
            return;
        };
        let current = self.agent_declarations().to_vec();
        let taken = declared_agent_ids(&current);
        let agent_id = suggest_agent_id(workspace, harness, &taken);
        let name = name.trim();
        let incoming = AgentDeclaration {
            agent_id: agent_id.clone(),
            host_id: self.local_host_id(),
            harness: harness.to_string(),
            workspace: WorkspaceRef::checkout(workspace),
            name: (!name.is_empty()).then(|| name.to_string()),
            roles: Vec::new(),
            // The only strategy an operator may choose in v1; a picker that
            // offered `worktree` would declare parallel sessions that all land in
            // one directory, because nothing provisions a worktree yet.
            strategy: WorkspaceStrategy::Checkout,
        };
        match declare_agent(&path, &current, incoming) {
            Ok(declarations) => {
                self.loaded.config.fleet.agent_declarations = declarations;
                self.select_agent_row(&agent_id);
                self.set_status(format!(
                    "Declared {agent_id} · {harness} in {workspace} · ^T opens a session"
                ));
            }
            // Nothing was applied — `declare_agent` writes before it returns, so
            // an error means the file is unchanged and so is the rail.
            Err(error) => self.set_status(format!("Could not declare the agent: {error}")),
        }
    }

    /// Offer to declare an agent for a session started somewhere undeclared.
    ///
    /// The quick path survives — the session is already running — but it always
    /// leaves a declared agent behind if the operator wants one, which is what
    /// keeps "declared, never discovered" true without making the fast door
    /// slower. Silent when the directory is already declared, and when this
    /// device is not hosting.
    pub(in crate::ui::app) fn offer_agent_declaration(&mut self, harness: &str, workspace: &str) {
        if self.harnesses.is_none() {
            return;
        }
        let declared = self.local_agent_declarations().into_iter().any(|held| {
            held.harness.trim().eq_ignore_ascii_case(harness.trim())
                && held.workspace.path.trim().trim_end_matches('/')
                    == workspace.trim().trim_end_matches('/')
        });
        if declared {
            return;
        }
        self.prompt_agent_name(harness, workspace);
    }

    /// The agent the rail cursor is on, or the one owning the session it is on.
    pub(in crate::ui::app) fn selected_agent_id(&self) -> Option<String> {
        let rows = self.rail_rows();
        rows.get(self.agent_index.min(rows.len().saturating_sub(1)))
            .and_then(|row| row.agent_id())
            .map(str::to_string)
    }

    /// Ask what to call a new session under `agent_id`, then open it.
    ///
    /// Managed-versus-unmanaged is ownership at birth and is asked first, in the
    /// same sentence, because it is the fact that decides whether the
    /// orchestrator may dispatch into what the operator is about to open.
    pub(in crate::ui::app) fn open_new_session(&mut self, agent_id: &str) {
        let Some(declaration) = self.declaration_for(agent_id) else {
            self.refuse_absent_declaration(agent_id);
            return;
        };
        let Some(workspace) = declaration.workspace.path() else {
            self.set_status(format!(
                "{agent_id} declares no workspace to open a session in"
            ));
            return;
        };
        self.prompt = Some(Prompt {
            title: format!(
                "Name this session — {} in {} (blank leaves it unnamed)",
                declaration.harness, workspace
            ),
            draft: Default::default(),
            kind: PromptKind::SessionName {
                agent_id: agent_id.to_string(),
                // Born to the operator: they asked for it and are about to type
                // into it, so the orchestrator does not get it until it is handed
                // over. The same default the manual picker offers.
                managed: false,
            },
        });
    }

    /// Open a session for `agent_id` with the name the operator gave it.
    ///
    /// The harness and the directory come from the declaration, never from the
    /// caller: a session that ran a different CLI, or in a different folder, from
    /// the agent it is filed under would put the rail's own grouping rule wrong.
    pub(in crate::ui::app) fn start_agent_session(
        &mut self,
        agent_id: &str,
        name: &str,
        managed: bool,
    ) {
        let Some(harnesses) = self.harnesses.clone() else {
            self.set_status("This device is not hosting, so it has no sessions to open");
            return;
        };
        let Some(declaration) = self.declaration_for(agent_id) else {
            self.refuse_absent_declaration(agent_id);
            return;
        };
        let Some(choice) = harness_choice(&harnesses.choices(), &declaration.harness) else {
            self.set_status(format!(
                "{} is not installed on this device — {agent_id} cannot open a session",
                declaration.harness
            ));
            return;
        };
        let workspace = declaration.workspace.path().unwrap_or_default().to_string();
        let name = name.trim();
        let name = (!name.is_empty()).then(|| name.to_string());
        let skip = self.harness_skip_permissions;
        match harnesses.open_unmanaged_named(&choice, &workspace, skip, name.clone()) {
            Ok(id) => {
                self.tab_index = tab_pos("Agents");
                self.select_session_row(&id);
                let label = name.unwrap_or_else(|| choice.display_name().to_string());
                if managed {
                    self.hand_back_session(&id, None);
                }
                self.set_status(format!(
                    "Opened {label} under {agent_id} · {}",
                    if managed {
                        "managed, the orchestrator may dispatch into it"
                    } else {
                        "yours, the orchestrator will not dispatch into it"
                    }
                ));
            }
            Err(error) => self.set_status(format!("Could not open a session: {error}")),
        }
    }

    /// The declaration for `agent_id` **on this host**, if there is one.
    ///
    /// Scoped to the local host on purpose: a session is a process started on
    /// this machine, so opening one from a declaration that names another
    /// machine would file a running session under an agent that lives somewhere
    /// else — and the rail, which resolves sessions against the *local*
    /// declarations, would then list it as an orphan. The rail already limits
    /// its `+ new session` row to local agents; `^T` reads whatever row the
    /// cursor is on, so the filter has to live here too.
    fn declaration_for(&self, agent_id: &str) -> Option<AgentDeclaration> {
        self.local_agent_declarations()
            .into_iter()
            .find(|declaration| declaration.agent_id.trim() == agent_id.trim())
    }

    /// Say why there is no declaration here to open a session from.
    ///
    /// Two different answers, because they call for two different next steps: an
    /// agent nobody declared anywhere is one to declare, and an agent declared
    /// on another machine is one to start a session on *that* machine. Naming
    /// the host is the whole of the difference, so the refusal carries it.
    fn refuse_absent_declaration(&mut self, agent_id: &str) {
        let elsewhere = medulla::config::agent_declaration(self.agent_declarations(), agent_id)
            .map(|declaration| declaration.host_id.trim().to_string())
            .filter(|host_id| !host_id.is_empty());
        self.set_status(match elsewhere {
            Some(host_id) => {
                format!("{agent_id} is declared on {host_id} — open its sessions on that machine")
            }
            None => format!("No agent \"{agent_id}\" is declared on this device"),
        });
    }

    /// Put the rail cursor on `agent_id`'s row, if it has one.
    pub(in crate::ui::app) fn select_agent_row(&mut self, agent_id: &str) {
        if let Some(index) = self.rail_rows().iter().position(
            |row| matches!(row, super::rail::RailRow::Agent(agent) if agent.agent_id == agent_id),
        ) {
            self.agent_index = index;
        }
    }

    /// Put the rail cursor on the row for `session_id`, if it has one.
    ///
    /// Selecting the new row matters more than it sounds: a session that appears
    /// somewhere below the fold, with the pane still showing whatever was
    /// selected before, reads as "nothing happened".
    pub(in crate::ui::app) fn select_session_row(&mut self, session_id: &str) {
        if let Some(index) = self
            .rail_rows()
            .iter()
            .position(|row| row.session_id() == Some(session_id))
        {
            self.agent_index = index;
        }
    }
}

/// The installed choice implementing `harness`, matched on its stable id.
///
/// The id is what a declaration records — a native CLI's wire name, or a custom
/// preset's own id — so this resolves both without the caller knowing which it
/// has.
fn harness_choice(choices: &[HarnessChoice], harness: &str) -> Option<HarnessChoice> {
    let wanted = harness.trim();
    choices
        .iter()
        .find(|choice| choice.id().eq_ignore_ascii_case(wanted))
        .cloned()
}
