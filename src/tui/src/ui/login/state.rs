//! The login-screen state machine: key handling
//! ([`LoginScreen::handle_key`]), async-event folding
//! ([`LoginScreen::apply`]), and the method → provider → flow progression.
//! Turns raw crossterm keys into [`LoginCmd`]s and folds [`LoginEvent`]s from
//! the async tasks back into screen state.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use medulla::auth::code_login_url;

use crate::ui::composer::flatten_paste;

use super::types::{
    LoginCmd, LoginEvent, LoginOutcome, LoginScreen, MenuItem, Phase, SignInMethod, DOCS_URL, MENU,
    PROVIDERS, REPO_URL,
};

impl LoginScreen {
    /// Move a wrapping list selection.
    ///
    /// Wrapping matters here: the lists are short and every row is reachable
    /// either way, so a user who overshoots Quit does not have to travel back up
    /// through the whole menu.
    fn move_index(index: &mut usize, len: usize, down: bool) {
        if len == 0 {
            return;
        }
        *index = if down {
            (*index + 1) % len
        } else {
            (*index + len - 1) % len
        };
    }

    /// Act on the highlighted Idle row.
    fn activate_menu(&mut self) -> Option<LoginCmd> {
        match MENU[self.menu_index.min(MENU.len() - 1)] {
            // Both sign-in methods take the same next step: pick a provider.
            // The method is recorded now so a retry after an error repeats it.
            MenuItem::Method(method) => {
                self.method = method;
                self.phase = Phase::ProviderPick;
                self.provider_index = 0;
                self.error = None;
                self.flash = None;
                None
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

    /// Start the chosen method's flow with the highlighted provider.
    ///
    /// The browser method hands off to an async task; the code method needs no
    /// I/O to begin — the verification URL is a pure function of the backend and
    /// the provider, so it is built here and shown immediately.
    fn activate_provider(&mut self) -> Option<LoginCmd> {
        let provider = PROVIDERS[self.provider_index.min(PROVIDERS.len() - 1)];
        self.provider = provider;
        self.error = None;
        self.flash = None;
        match self.method {
            SignInMethod::Browser => {
                self.phase = Phase::Starting;
                Some(LoginCmd::StartLoopback {
                    base_url: self.base_url.clone(),
                    provider,
                })
            }
            SignInMethod::Code => {
                self.phase = Phase::CodeEntry;
                self.url = Some(code_login_url(&self.base_url, provider));
                self.input.clear();
                None
            }
        }
    }

    /// Submit whatever is in the single-line input, or refuse an empty one.
    ///
    /// Shared by the code and API-key phases: both end in the same
    /// [`LoginCmd::SubmitToken`], which classifies a 64-hex value as a one-time
    /// code to redeem and anything else as a ready-made JWT or key.
    fn submit_input(&mut self, empty_error: &str) -> Option<LoginCmd> {
        let token = self.input.trim().to_string();
        if token.is_empty() {
            self.error = Some(empty_error.to_string());
            return None;
        }
        self.input.clear();
        self.error = None;
        self.flash = None;
        self.resume_phase = self.phase;
        self.phase = Phase::Verifying;
        Some(LoginCmd::SubmitToken(token))
    }

    /// Append a bracketed-paste payload to the active single-line input.
    ///
    /// The code and API-key phases exist to be pasted into, and with bracketed
    /// paste enabled the value no longer arrives as key presses — without this
    /// it would be dropped entirely, and a code copied with a trailing newline
    /// would previously have submitted itself half-typed. Line breaks flatten to
    /// spaces, which the `trim` on submit then removes. Ignored in every other
    /// phase: none of them has a field to paste into.
    pub fn handle_paste(&mut self, text: &str) {
        if !matches!(self.phase, Phase::TokenEntry | Phase::CodeEntry) {
            return;
        }
        self.input.push_str(&flatten_paste(text));
    }

    /// Handle one key event, optionally emitting an async command.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<LoginCmd> {
        // Ctrl-C quits from anywhere.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.outcome = Some(LoginOutcome::Quit);
            return None;
        }

        match self.phase {
            // The code phase is a text field, so the "open this URL here" escape
            // hatch has to be a modifier chord — every plain character belongs
            // to the code being entered. It is best-effort by design: the whole
            // point of this flow is that there may be no browser to open.
            Phase::CodeEntry
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('o') =>
            {
                let url = self.url.clone()?;
                self.flash = Some(format!("opened {url}"));
                Some(LoginCmd::OpenUrl(url))
            }
            Phase::CodeEntry => match key.code {
                // Back to the provider list rather than the top menu: a code
                // that will not redeem is usually the wrong account, and the fix
                // is another provider.
                KeyCode::Esc => {
                    self.phase = Phase::ProviderPick;
                    self.input.clear();
                    self.url = None;
                    self.error = None;
                    None
                }
                KeyCode::Enter => self.submit_input("paste the code from the browser first"),
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
            Phase::TokenEntry => match key.code {
                KeyCode::Esc => {
                    self.phase = Phase::Idle;
                    self.input.clear();
                    None
                }
                KeyCode::Enter => self.submit_input("enter a token first"),
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
            Phase::ProviderPick => match key.code {
                KeyCode::Esc => {
                    self.phase = Phase::Idle;
                    None
                }
                KeyCode::Up => {
                    Self::move_index(&mut self.provider_index, PROVIDERS.len(), false);
                    None
                }
                KeyCode::Down => {
                    Self::move_index(&mut self.provider_index, PROVIDERS.len(), true);
                    None
                }
                KeyCode::Enter => self.activate_provider(),
                _ => None,
            },
            Phase::Starting | Phase::Waiting => match key.code {
                KeyCode::Esc => {
                    self.phase = Phase::ProviderPick;
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
                    Self::move_index(&mut self.menu_index, MENU.len(), false);
                    None
                }
                KeyCode::Down => {
                    Self::move_index(&mut self.menu_index, MENU.len(), true);
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
                self.resume_phase = Phase::Idle;
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
            // Back to whichever input produced the value, not to the top menu: a
            // rejected code or key is retried by fetching or pasting another
            // one, and dropping the operator on the menu would hide the URL
            // they need to do it.
            LoginEvent::VerifyFailed(msg) => {
                self.phase = self.resume_phase;
                self.error = Some(msg);
            }
        }
    }
}
