//! Keyboard handling for the Add Host wizard.
//!
//! Split from the Routing key module because it is a multi-step flow with its
//! own state — which kind, whether that kind is settled, which harness — rather
//! than the single-key actions its sibling panes use, and `mod.rs` is for
//! wiring.

use crossterm::event::KeyCode;

use crate::ui::composer::Draft;
use medulla::daemon::pairing::REMOTE_JOIN_COMMAND;

use super::super::super::types::{AddHostKind, App, Prompt, PromptKind};
use super::RoutingKey;

impl App {
    /// Open the existing host-address prompt from the dedicated Add Host pane,
    /// or copy the line the operator has to run on the machine being added.
    pub(super) fn add_host_key(&mut self, code: KeyCode) -> RoutingKey {
        match code {
            // The arrows move whichever list the page is currently showing:
            // the kind picker until one is chosen, the harness list after. One
            // pair of keys for a page that is read top to bottom.
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                let up = matches!(code, KeyCode::Up | KeyCode::Char('k'));
                // A confirmed kind is settled — the wizard has moved past it and
                // the arrows belong to the live step. Letting them keep driving
                // the kind list meant confirming Remote and arrowing to Local
                // carried the confirmation across, so the next Enter skipped
                // "Choose a harness" and asked for a directory for a harness
                // nobody had picked. Esc is how you go back a step.
                match (self.add_host_selected_kind(), self.add_host_kind_chosen) {
                    (AddHostKind::Local, true) => {
                        let len = self.add_host_providers().len();
                        self.add_host_harness = crate::ui::selection::moved(
                            self.add_host_harness.min(len.saturating_sub(1)),
                            len,
                            up,
                        );
                    }
                    // Remote's confirmed step is instructions, with nothing to
                    // move through. Consumed rather than passed on, so an arrow
                    // here cannot silently reopen the kind choice.
                    (_, true) => {}
                    (_, false) => {
                        self.add_host_kind = crate::ui::selection::moved(
                            self.add_host_kind.min(AddHostKind::ALL.len() - 1),
                            AddHostKind::ALL.len(),
                            up,
                        );
                    }
                }
                RoutingKey::Handled(None)
            }
            KeyCode::Enter | KeyCode::Char('a') => {
                match self.add_host_selected_kind() {
                    // Both kinds settle the choice first, so the page always
                    // reads the same way: pick a kind, then the steps it needs
                    // light up. Remote's are instructions rather than a list,
                    // but jumping straight to the prompt made the same key mean
                    // two different things depending on which row was under it.
                    AddHostKind::Remote if !self.add_host_kind_chosen => {
                        self.add_host_kind_chosen = true;
                        self.set_status(
                            "Run the line on that machine · Enter to paste its address",
                        );
                    }
                    AddHostKind::Remote => {
                        self.add_host_kind_chosen = false;
                        self.open_add_host_prompt();
                    }
                    // Two Enters for a local host: the first settles the
                    // harness, the second asks where it works. Collapsing them
                    // would mean the arrows never reached the harness list.
                    AddHostKind::Local if !self.add_host_kind_chosen => {
                        self.add_host_kind_chosen = true;
                        self.set_status("Choose a harness · Enter to set the directory");
                    }
                    AddHostKind::Local => {
                        let providers = self.add_host_providers();
                        let harness = providers[self.add_host_harness.min(providers.len() - 1)];
                        self.prompt = Some(Prompt {
                            kind: PromptKind::LocalHostWorkspace(harness),
                            title: format!(
                                "Directory for the {} host — blank uses this one",
                                harness.as_str()
                            ),
                            draft: Draft::new(),
                        });
                        self.add_host_kind_chosen = false;
                        self.set_status("Add local host · Enter save · Esc cancel");
                    }
                }
                RoutingKey::Handled(None)
            }
            // Copying it here rather than retyping it there is the whole point:
            // this end is a local terminal, so the copy is free. Remote only —
            // there is no install line in the local flow, and the hint no longer
            // offers one.
            KeyCode::Char('c') if self.add_host_selected_kind() == AddHostKind::Remote => {
                self.copy_line("the worker install line", REMOTE_JOIN_COMMAND);
                RoutingKey::Handled(None)
            }
            _ => RoutingKey::Unhandled,
        }
    }
}
