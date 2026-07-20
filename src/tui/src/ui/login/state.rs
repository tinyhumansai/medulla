//! The login-screen state machine: key handling
//! ([`LoginScreen::handle_key`]), async-event folding
//! ([`LoginScreen::apply`]), and provider cycling. Turns raw crossterm keys
//! into [`LoginCmd`]s and folds [`LoginEvent`]s from the async tasks back into
//! screen state.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::types::{
    LoginCmd, LoginEvent, LoginOutcome, LoginScreen, MenuItem, Phase, DOCS_URL, MENU, REPO_URL,
};

impl LoginScreen {
    /// Move the Idle menu highlight, wrapping at both ends.
    ///
    /// Wrapping matters here: the list is short and every row is reachable
    /// either way, so a user who overshoots Quit does not have to travel back up
    /// through the whole menu.
    fn move_menu(&mut self, down: bool) {
        let len = MENU.len();
        self.menu_index = if down {
            (self.menu_index + 1) % len
        } else {
            (self.menu_index + len - 1) % len
        };
    }

    /// Act on the highlighted menu row.
    fn activate_menu(&mut self) -> Option<LoginCmd> {
        match MENU[self.menu_index.min(MENU.len() - 1)] {
            MenuItem::Provider(provider) => {
                // Record the choice so a retry after an error reuses it.
                self.provider = provider;
                self.phase = Phase::Starting;
                self.error = None;
                self.flash = None;
                Some(LoginCmd::StartLoopback {
                    base_url: self.base_url.clone(),
                    provider,
                })
            }
            MenuItem::PasteKey => {
                self.phase = Phase::TokenEntry;
                self.input.clear();
                self.error = None;
                self.flash = None;
                None
            }
            // Link rows open a browser tab and leave the menu exactly as it
            // was: reading the docs is not a way of answering "how do I sign
            // in", so it must not disturb the sign-in you are part-way through.
            MenuItem::Docs => {
                self.flash = Some(format!("opened {DOCS_URL}"));
                self.error = None;
                Some(LoginCmd::OpenUrl(DOCS_URL.to_string()))
            }
            MenuItem::Star => {
                self.flash = Some(format!("opened {REPO_URL}"));
                self.error = None;
                Some(LoginCmd::OpenUrl(REPO_URL.to_string()))
            }
            MenuItem::Quit => {
                self.outcome = Some(LoginOutcome::Quit);
                None
            }
        }
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
            // Every Idle action is a row in one menu: arrows move, Enter picks.
            // Nothing here is bound to a letter, so there is no shortcut to
            // learn and no keystroke that fires an action by surprise.
            Phase::Idle => match key.code {
                KeyCode::Up => {
                    self.move_menu(false);
                    None
                }
                KeyCode::Down => {
                    self.move_menu(true);
                    None
                }
                KeyCode::Enter => self.activate_menu(),
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
