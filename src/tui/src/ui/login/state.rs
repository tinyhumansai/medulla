//! The login-screen state machine: key handling
//! ([`LoginScreen::handle_key`]), async-event folding
//! ([`LoginScreen::apply`]), and provider cycling. Turns raw crossterm keys
//! into [`LoginCmd`]s and folds [`LoginEvent`]s from the async tasks back into
//! screen state.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use medulla::auth::Provider;

use super::types::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen, Phase};

/// The four providers, in the order the panel cycles through them.
const PROVIDERS: [Provider; 4] = [
    Provider::Google,
    Provider::Github,
    Provider::Twitter,
    Provider::Discord,
];

impl LoginScreen {
    fn cycle_provider(&mut self, forward: bool) {
        let pos = PROVIDERS
            .iter()
            .position(|p| *p == self.provider)
            .unwrap_or(0);
        let len = PROVIDERS.len();
        let next = if forward {
            (pos + 1) % len
        } else {
            (pos + len - 1) % len
        };
        self.provider = PROVIDERS[next];
    }

    /// Handle one key event, optionally emitting an async command.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<LoginCmd> {
        // Ctrl-C quits from anywhere.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.outcome = Some(LoginOutcome::Quit);
            return None;
        }

        match self.phase {
            Phase::TokenEntry => match key.code {
                KeyCode::Esc => {
                    self.phase = Phase::Idle;
                    self.input.clear();
                    None
                }
                KeyCode::Enter => {
                    let token = self.input.trim().to_string();
                    if token.is_empty() {
                        self.error = Some("enter a token first".into());
                        return None;
                    }
                    self.input.clear();
                    self.error = None;
                    self.flash = None;
                    self.phase = Phase::Verifying;
                    Some(LoginCmd::SubmitToken(token))
                }
                KeyCode::Backspace => {
                    self.input.pop();
                    None
                }
                KeyCode::Char(c) => {
                    self.input.push(c);
                    None
                }
                _ => None,
            },
            Phase::Starting | Phase::Waiting => match key.code {
                KeyCode::Esc => {
                    self.phase = Phase::Idle;
                    self.url = None;
                    self.port = None;
                    self.error = None;
                    Some(LoginCmd::CancelLoopback)
                }
                _ => None,
            },
            Phase::Verifying => None,
            Phase::Idle => match key.code {
                KeyCode::Enter | KeyCode::Char('o') => {
                    self.phase = Phase::Starting;
                    self.error = None;
                    self.flash = None;
                    Some(LoginCmd::StartLoopback {
                        base_url: self.base_url.clone(),
                        provider: self.provider,
                    })
                }
                KeyCode::Char('t') => {
                    self.phase = Phase::TokenEntry;
                    self.input.clear();
                    self.error = None;
                    self.flash = None;
                    None
                }
                KeyCode::Char('m') => {
                    self.outcome = Some(LoginOutcome::Mock);
                    None
                }
                KeyCode::Char('q') => {
                    self.outcome = Some(LoginOutcome::Quit);
                    None
                }
                KeyCode::Right | KeyCode::Char('p') => {
                    self.cycle_provider(true);
                    None
                }
                KeyCode::Left => {
                    self.cycle_provider(false);
                    None
                }
                _ => None,
            },
        }
    }

    /// Fold an async event back into screen state.
    pub fn apply(&mut self, ev: LoginEvent) {
        match ev {
            LoginEvent::LoopbackStarted { url, port } => {
                self.phase = Phase::Waiting;
                self.url = Some(url);
                self.port = Some(port);
                self.error = None;
            }
            LoginEvent::CallbackToken(_) => {
                self.phase = Phase::Verifying;
                self.flash = Some("callback received — verifying…".into());
                self.error = None;
            }
            LoginEvent::CallbackError(msg) => {
                self.phase = Phase::Idle;
                self.url = None;
                self.port = None;
                self.error = Some(msg);
            }
            LoginEvent::Verified { jwt, who } => {
                // `who` is the `describe_me` summary, already phrased
                // "Logged in as …".
                self.flash = Some(who);
                self.error = None;
                self.outcome = Some(LoginOutcome::Token(jwt));
            }
            LoginEvent::VerifyFailed(msg) => {
                self.phase = Phase::Idle;
                self.error = Some(msg);
            }
        }
    }
}
