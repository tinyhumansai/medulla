//! Starting local harness sessions and presenting the harness-type picker.
//!
//! This module owns both picker state transitions and workspace-key routing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::protocol::HarnessProvider;

use crate::ui::harness_pane::HarnessChoice;

use super::super::types::Cmd;
use super::super::types::{
    tab_pos, App, PickerHost, Prompt, PromptKind, SessionPicker, SessionPickerStep,
};
use crate::ui::composer::Draft;

impl App {
    /// Open the "start a session" picker, or spawn directly when the command
    /// already named a harness type.
    ///
    /// `/session` with no harness type opens the picker rather than guessing:
    /// starting the wrong CLI in the operator's workspace is not something they
    /// find out about until it has already done something.
    pub(crate) fn start_session_command(&mut self, provider: Option<&str>, path: Option<&str>) {
        match provider.and_then(HarnessProvider::from_wire) {
            // Naming a provider names one on *this* device: `/session claude`
            // says which CLI, not which machine. A remote session is chosen in
            // the picker, where there is a host step to choose it on.
            Some(provider) => {
                if self.local_sessions.is_none() {
                    self.set_status("This device is not hosting, so it has no sessions to start");
                    return;
                }
                let cwd = path.unwrap_or("").to_string();
                self.spawn_session(HarnessChoice::native(provider), &cwd);
            }
            None => {
                // Built from whatever hosts exist, local or not. A client run
                // with `MEDULLA_HOST=0` hosts nothing and still has every
                // configured `[[remoteHosts]]` machine available — returning
                // early on a missing local surface made those unreachable, which
                // is the one configuration where they are the *only* thing there
                // is to reach.
                let hosts = self.picker_hosts();
                let Some(first) = hosts.first().cloned() else {
                    self.set_status(
                        "This device is not hosting and no [[remoteHosts]] are configured",
                    );
                    return;
                };
                let choices = match first.remote {
                    false => self
                        .local_sessions
                        .as_ref()
                        .map(|local| local.choices())
                        .unwrap_or_default(),
                    // Empty until the host answers; the host step is where it is
                    // dialled, which is why a lone remote host still starts there.
                    true => self.remote_choices(&first.id),
                };
                if !first.remote && choices.is_empty() {
                    self.set_status("No harness CLIs found on this device");
                    return;
                }
                // Decides the *starting step* rather than gating a branch inside
                // each handler. The harness step is reachable directly only when
                // the single host is this device — with no `[[remoteHosts]]` the
                // vec has one local entry, the step is `Harness`, and the
                // ordinary case gains not a single keystroke. A remote host has
                // to be dialled before its harnesses are known, and dialling
                // happens on the host step, so a lone remote host starts there.
                let step = match hosts.len() > 1 || first.remote {
                    true => SessionPickerStep::Host,
                    false => SessionPickerStep::Harness,
                };
                self.session_picker = Some(SessionPicker {
                    hosts,
                    host_index: 0,
                    choices,
                    index: 0,
                    step,
                    cwd: path
                        .map(str::to_string)
                        .unwrap_or_else(|| first.workspace.clone()),
                    workspace_query: String::new(),
                    workspace_choices: Vec::new(),
                    workspace_index: 0,
                    workspace_picked: false,
                });
            }
        }
    }

    /// Open the picker from the keyboard shortcut.
    pub(crate) fn open_session_picker(&mut self) {
        self.start_session_command(None, None);
    }

    /// Every machine a session could start on, this device first.
    ///
    /// This device leads because it is where a session goes when nobody says
    /// otherwise, and the picker's first row is what `Enter` takes. A disabled or
    /// half-written `[[remoteHosts]]` entry is already dropped by
    /// [`remote_hosts`](medulla::config::remote_hosts), so every row here names a
    /// host that can actually be dialled.
    pub(in crate::ui::app) fn picker_hosts(&self) -> Vec<PickerHost> {
        // Offered only when this device can actually start something. A client
        // run with `MEDULLA_HOST=0` hosts nothing, and a row that could only
        // fail when chosen is worse than no row.
        let local = self.local_sessions.as_ref();
        local
            .map(|local| PickerHost::this_device(local.workspace.clone()))
            .into_iter()
            .chain(
                medulla::config::remote_hosts(&self.loaded.config.remote_hosts)
                    .into_iter()
                    .map(|host| PickerHost {
                        label: match host.name.trim() {
                            "" => host.id.clone(),
                            name => name.to_string(),
                        },
                        id: host.id,
                        workspace: host.workspace,
                        remote: true,
                    }),
            )
            .collect()
    }

    /// The host the picker is currently pointed at.
    pub(in crate::ui::app) fn picker_host(&self) -> Option<&PickerHost> {
        let picker = self.session_picker.as_ref()?;
        picker.hosts.get(picker.host_index)
    }

    /// Advance the picker now that `host_id` has answered.
    ///
    /// Called when a connection completes. The operator pressed Enter on this
    /// host and has been waiting on exactly this answer, so the picker moves
    /// itself along — making them press Enter a second time would be asking for
    /// a decision they already made.
    ///
    /// Does nothing unless the picker is still open, still on the host step, and
    /// still pointed at the host that answered: any of those changing means the
    /// operator has moved on, and moving the picker under them would be worse
    /// than leaving it.
    pub fn resume_picker_host(&mut self, host_id: &str) {
        let still_waiting = self.session_picker.as_ref().is_some_and(|picker| {
            picker.step == SessionPickerStep::Host
                && picker
                    .hosts
                    .get(picker.host_index)
                    .is_some_and(|host| host.id == host_id)
        });
        if still_waiting {
            self.confirm_picker_host();
        }
    }

    /// Route a key while the host step owns the keyboard.
    fn handle_picker_host_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc => {
                self.session_picker = None;
                self.set_status("Cancelled");
            }
            KeyCode::Up => {
                if let Some(picker) = &mut self.session_picker {
                    picker.host_index = picker.host_index.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.session_picker {
                    picker.host_index =
                        (picker.host_index + 1).min(picker.hosts.len().saturating_sub(1));
                }
            }
            KeyCode::Enter => self.confirm_picker_host(),
            _ => {}
        }
    }

    /// Move from the host step to the harness step.
    ///
    /// The chosen host decides both halves of what comes next: which harnesses
    /// are on offer, and which directory the workspace step starts from. A
    /// remote host's list comes from the host itself rather than from this
    /// machine's — offering a `claude` row for a box that has none would break
    /// the picker's standing promise that it never shows a row that cannot
    /// start.
    pub(in crate::ui::app) fn confirm_picker_host(&mut self) {
        let Some(host) = self.picker_host().cloned() else {
            return;
        };
        if host.remote {
            // Dialled here rather than at startup, which is the whole of "opened
            // when needed": a configured host that is switched off, or on a
            // network this laptop is not on, costs nothing until somebody asks
            // for it. The picker stays on this step while `ssh` runs — the
            // harness list has to come from the host, and there is nothing
            // truthful to show until it answers.
            let should_dial = self
                .remote_host(&host.id)
                .map(|state| state.should_dial())
                .unwrap_or(true);
            if should_dial {
                self.remote_host_connecting(&host.id);
                self.queue_cmd(Cmd::ConnectRemoteHost {
                    host_id: host.id.clone(),
                });
                self.set_status(format!("Connecting to {}…", host.label));
                return;
            }
            if !matches!(
                self.remote_host(&host.id).map(|state| state.status.clone()),
                Some(crate::ui::app::remote_hosts::RemoteHostStatus::Live)
            ) {
                self.set_status(format!("{} is still connecting…", host.label));
                return;
            }
        }
        let choices = match host.remote {
            false => self
                .local_sessions
                .as_ref()
                .map(|local| local.choices())
                .unwrap_or_default(),
            true => self.remote_choices(&host.id),
        };
        if choices.is_empty() {
            // Named, because "nothing happened" is the worst possible answer to
            // pressing Enter on a machine.
            self.set_status(format!("{} offers nothing to start", host.label));
            return;
        }
        // The operator's `[[remoteHosts]] workspace` wins when they set one: it
        // is an explicit choice about *this* host. Otherwise the host's own
        // advertised default, because it knows where its sessions should start
        // and this machine does not. Blank would be neither, and would start the
        // workspace step with nothing in it.
        let cwd = match host.workspace.trim() {
            "" => self
                .remote_host(&host.id)
                .map(|state| state.capabilities.workspace.clone())
                .unwrap_or_default(),
            configured => configured.to_string(),
        };
        if let Some(picker) = &mut self.session_picker {
            picker.choices = choices;
            picker.index = 0;
            picker.cwd = cwd;
            picker.workspace_query = String::new();
            picker.workspace_choices = Vec::new();
            picker.workspace_index = 0;
            picker.workspace_picked = false;
            picker.step = SessionPickerStep::Harness;
        }
    }

    /// Start a session the operator owns and move the cursor onto it.
    ///
    /// Always unmanaged, and not as a default the operator can override: a
    /// session started by hand is one somebody intends to type into, and the
    /// orchestrator starts its own managed without being asked. Spawning one
    /// into dispatch would mean the very next thing the operator does — press
    /// Enter on the row they just created — is a request to take it back off
    /// the orchestrator it was handed to a keystroke earlier.
    ///
    /// Selecting the new row matters more than it sounds: a session that
    /// appears somewhere below the fold, with the pane still showing whatever
    /// was selected before, reads as "nothing happened".
    pub(crate) fn spawn_session(&mut self, choice: HarnessChoice, cwd: &str) {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions to start");
            return;
        };
        let skip = self.harness_skip_permissions;
        let workspace = harnesses.resolve_workspace(cwd);
        match harnesses.open_unmanaged(&choice, &workspace, skip) {
            Ok(id) => {
                self.tab_index = tab_pos("Sessions");
                self.select_session_row(&id);
                // Names the directory, because the picker's whole second step
                // was choosing it and a confirmation that omits the answer is
                // not one. "local session" covers a shell as squarely as it
                // covers a harness: it states where the session runs and who
                // holds it, not whether the orchestrator declined to dispatch
                // into it, so a shell needs no wording of its own here.
                let mut status = format!(
                    "Started {} in {workspace} · local session",
                    choice.display_name()
                );
                if let Err(error) = self.remember_harness_workspace(&workspace) {
                    status.push_str(&format!(" · {error}"));
                }
                self.set_status(status);
            }
            // Surfaced, never swallowed: a spawn that fails silently leaves the
            // operator waiting for a pane that is never coming.
            Err(err) => {
                self.set_status(format!("Could not start {}: {err}", choice.display_name()))
            }
        }
    }

    /// Route a key while the harness picker is open.
    pub(crate) fn handle_session_picker_key(&mut self, event: KeyEvent) {
        let step = self
            .session_picker
            .as_ref()
            .map(|picker| picker.step)
            .unwrap_or(SessionPickerStep::Harness);
        if step == SessionPickerStep::Host {
            self.handle_picker_host_key(event);
            return;
        }
        if step == SessionPickerStep::Workspace {
            self.handle_harness_workspace_key(event);
            return;
        }
        match event.code {
            // Back to the host step when one ran, and closed when none did.
            // Somebody who picked the wrong machine should not have to reopen
            // the modal to change it — and with a single host there is nothing
            // behind this step to go back to, so Esc keeps meaning "cancel"
            // exactly as it always has.
            KeyCode::Esc | KeyCode::BackTab => {
                let has_host_step = self
                    .session_picker
                    .as_ref()
                    .is_some_and(|picker| picker.hosts.len() > 1);
                match has_host_step {
                    true => {
                        if let Some(picker) = &mut self.session_picker {
                            picker.step = SessionPickerStep::Host;
                        }
                        self.set_status("Pick a host · Enter agent · Esc cancel");
                    }
                    false => {
                        self.session_picker = None;
                        self.set_status("Cancelled");
                    }
                }
            }
            KeyCode::Up => {
                if let Some(picker) = &mut self.session_picker {
                    picker.index = picker.index.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.session_picker {
                    picker.index = (picker.index + 1).min(picker.choices.len().saturating_sub(1));
                }
            }
            KeyCode::Char('e') if is_text_input(event.modifiers) => {
                self.open_harness_workspace_step(true)
            }
            KeyCode::Enter => self.open_harness_workspace_step(false),
            _ => {}
        }
    }

    /// Route a key while completing a workspace directory.
    fn handle_harness_workspace_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc | KeyCode::BackTab => {
                if let Some(picker) = &mut self.session_picker {
                    picker.step = SessionPickerStep::Harness;
                }
                self.set_status("Pick a session type · Enter workspace · Esc cancel");
            }
            KeyCode::Up => {
                if let Some(picker) = &mut self.session_picker {
                    picker.workspace_index = picker.workspace_index.saturating_sub(1);
                    picker.workspace_picked = !picker.workspace_choices.is_empty();
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.session_picker {
                    picker.workspace_index = (picker.workspace_index + 1)
                        .min(picker.workspace_choices.len().saturating_sub(1));
                    picker.workspace_picked = !picker.workspace_choices.is_empty();
                }
            }
            KeyCode::Tab => self.complete_harness_workspace(),
            KeyCode::Char('F') if event.modifiers == KeyModifiers::SHIFT => {
                let Some(workspace) = self.selected_picker_workspace() else {
                    self.set_status("Choose an existing directory before saving a favorite");
                    return;
                };
                self.prompt = Some(Prompt {
                    kind: PromptKind::FavoriteWorkspaceAdd(workspace.clone()),
                    title: format!("Save favorite for {workspace}"),
                    draft: Draft::new(),
                });
                self.set_status("Favorite name · Enter save · Esc cancel");
            }
            KeyCode::Backspace => {
                if let Some(picker) = &mut self.session_picker {
                    picker.workspace_query.pop();
                    picker.workspace_index = 0;
                    picker.workspace_picked = false;
                }
                self.refresh_harness_workspace_choices();
            }
            KeyCode::Char(character) if is_text_input(event.modifiers) => {
                if let Some(picker) = &mut self.session_picker {
                    picker.workspace_query.push(character);
                    picker.workspace_index = 0;
                    picker.workspace_picked = false;
                }
                self.refresh_harness_workspace_choices();
            }
            KeyCode::Enter => {
                let Some(workspace) = self.selected_picker_workspace() else {
                    self.set_status("Choose an existing directory");
                    return;
                };
                let Some(choice) = self
                    .session_picker
                    .as_ref()
                    .and_then(|picker| picker.choices.get(picker.index).cloned())
                else {
                    self.set_status("Choose a session type first");
                    return;
                };
                // Read *before* the picker is cleared: `spawn_session` asks it
                // which machine the session is for, and a cleared picker answers
                // "this device" — which is how a remote launch silently became a
                // local one.
                let host = self.picker_host().cloned();
                self.session_picker = None;
                match host.filter(|host| host.remote) {
                    Some(host) => {
                        self.spawn_remote_session(&host.id, &host.label, &choice, &workspace)
                    }
                    None => self.spawn_session(choice, &workspace),
                }
            }
            _ => {}
        }
    }
}

/// Return whether modifiers represent ordinary printable text input.
pub(in crate::ui::app) fn is_text_input(modifiers: KeyModifiers) -> bool {
    modifiers == KeyModifiers::NONE
        || modifiers == KeyModifiers::SHIFT
        || modifiers == (KeyModifiers::CONTROL | KeyModifiers::ALT)
}

impl App {
    /// Ask a remote host to start a session, and close the picker.
    ///
    /// The answer arrives asynchronously — the request goes up the link and the
    /// host replies with the id it minted — so there is nothing to select yet.
    /// The status line says what was asked for; `select_remote_session` moves
    /// the cursor when it lands.
    pub(in crate::ui::app) fn spawn_remote_session(
        &mut self,
        host_id: &str,
        host_label: &str,
        choice: &HarnessChoice,
        cwd: &str,
    ) {
        // The pane's size, so the far side's pty starts at the geometry it will
        // actually live in rather than starting at a default and reflowing on
        // the first frame the operator sees.
        let (cols, rows) = self
            .remote_host(host_id)
            .and_then(|state| state.pane_size)
            .unwrap_or((120, 30));
        let workspace = cwd.trim().to_string();
        let request = crate::remote::client::RemoteRequest::Open {
            request_id: String::new(),
            harness_id: choice.id().to_string(),
            workspace: workspace.clone(),
            cols,
            rows,
            name: None,
        };
        let sent = self
            .remote_hosts
            .get_mut(host_id)
            .map(|state| state.request(request))
            .unwrap_or_else(|| Err(format!("{host_label} is not connected")));
        self.session_picker = None;
        match sent {
            Ok(()) => {
                let where_ = match workspace.is_empty() {
                    true => host_label.to_string(),
                    false => format!("{workspace} on {host_label}"),
                };
                self.set_status(format!("Starting {} in {where_}…", choice.display_name()));
            }
            // Surfaced, never swallowed: a spawn that fails silently leaves the
            // operator waiting for a pane that is never coming.
            Err(error) => self.set_status(format!(
                "Could not start {} on {host_label}: {error}",
                choice.display_name()
            )),
        }
    }
}
