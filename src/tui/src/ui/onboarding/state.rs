//! The onboarding screen's state machine: [`OnboardingScreen::handle_key`] folds
//! key events into step transitions and optional async [`OnboardingCmd`]s, and
//! [`OnboardingScreen::apply`] folds [`OnboardingEvent`]s from spawned work back
//! into state. Rendering lives in [`super::draw`]; the data model in
//! [`super::types`].

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::types::{OnboardingCmd, OnboardingEvent, OnboardingOutcome, OnboardingScreen, Step};

impl OnboardingScreen {
    /// Handle one key event, optionally emitting an async command.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<OnboardingCmd> {
        // Ctrl-C aborts from anywhere.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.outcome = Some(OnboardingOutcome::Abort);
            return None;
        }

        match self.step {
            Step::Name => match key.code {
                KeyCode::Enter => {
                    let name = self.name.trim().to_string();
                    if name.is_empty() {
                        self.error = Some("enter a worker name".into());
                        return None;
                    }
                    self.name = name.clone();
                    self.error = None;
                    self.step = Step::Connecting;
                    Some(OnboardingCmd::LoadIdentity { name })
                }
                KeyCode::Backspace => {
                    self.name.pop();
                    None
                }
                KeyCode::Char(c) => {
                    self.name.push(c);
                    None
                }
                _ => None,
            },
            Step::Connecting => None,
            Step::Owner => match key.code {
                KeyCode::Enter => {
                    self.error = None;
                    self.flash = None;
                    self.step = Step::Confirm;
                    None
                }
                KeyCode::Esc => {
                    // Skip: forget any typed owner, note it can be set later.
                    self.owner.clear();
                    self.flash = Some("no owner set — you can set one later".into());
                    self.step = Step::Confirm;
                    None
                }
                KeyCode::Backspace => {
                    self.owner.pop();
                    None
                }
                KeyCode::Char(c) => {
                    self.owner.push(c);
                    None
                }
                _ => None,
            },
            Step::Confirm => match key.code {
                KeyCode::Enter => {
                    let owner = self.owner.trim();
                    self.outcome = Some(OnboardingOutcome::Register {
                        name: self.name.clone(),
                        owner: (!owner.is_empty()).then(|| owner.to_string()),
                    });
                    None
                }
                KeyCode::Char('q') => {
                    self.outcome = Some(OnboardingOutcome::Abort);
                    None
                }
                KeyCode::Esc => {
                    // Back to editing the owner.
                    self.step = Step::Owner;
                    self.flash = None;
                    None
                }
                _ => None,
            },
        }
    }

    /// Fold an async event back into screen state.
    pub fn apply(&mut self, ev: OnboardingEvent) {
        match ev {
            OnboardingEvent::IdentityReady { address, handle } => {
                self.address = Some(address);
                self.handle = handle;
                self.error = None;
                if self.step == Step::Connecting {
                    self.step = Step::Owner;
                }
            }
            OnboardingEvent::IdentityFailed(msg) => {
                self.error = Some(msg);
                // Return to the name step so the operator can retry.
                self.step = Step::Name;
            }
        }
    }
}
