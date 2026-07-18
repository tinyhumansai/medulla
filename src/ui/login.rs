//! The pre-app login screen: a pure state machine the `main` pre-app loop drives
//! before the main TUI when the backend runtime needs a token.
//!
//! All async work (binding the loopback listener, opening the browser, awaiting
//! the callback, redeeming a one-time token, and `me()` verification) lives in
//! `main`. This module only owns state and rendering: [`LoginScreen::handle_key`]
//! turns keys into [`LoginCmd`]s, [`LoginScreen::apply`] folds [`LoginEvent`]s
//! from those async tasks back into state, and [`LoginScreen::draw`] renders the
//! centered panel. The loop reads [`LoginScreen::outcome`] to know when to stop.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::auth::Provider;
use crate::ui::app::SPINNER;

/// The four providers, in the order the panel cycles through them.
const PROVIDERS: [Provider; 4] = [
    Provider::Google,
    Provider::Github,
    Provider::Twitter,
    Provider::Discord,
];

/// The terminal outcome of the login screen, consumed by the `main` pre-app loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    /// A verified JWT — proceed into the main app with a backend runtime.
    Token(String),
    /// Continue offline with the mock runtime.
    Mock,
    /// Quit cleanly without starting the app.
    Quit,
}

/// An async action the pre-app loop must run on the screen's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginCmd {
    /// Bind the loopback listener, open the browser, and await the callback.
    StartLoopback {
        base_url: String,
        provider: Provider,
    },
    /// Abort a running loopback task (Esc while waiting).
    CancelLoopback,
    /// Redeem/verify a pasted JWT or 64-hex one-time token.
    SubmitToken(String),
}

/// An event fed back from a spawned async task into [`LoginScreen::apply`].
#[derive(Debug, Clone)]
pub enum LoginEvent {
    /// The loopback listener is bound; show the URL and waiting spinner.
    LoopbackStarted { url: String, port: u16 },
    /// A JWT was captured from the loopback callback (verification pending).
    CallbackToken(String),
    /// The loopback flow failed (backend error, state-mismatch timeout, …).
    CallbackError(String),
    /// A JWT was verified via `me()`; `who` is the `describe_me` summary.
    Verified { jwt: String, who: String },
    /// Verification (or token redemption) failed.
    VerifyFailed(String),
}

/// Where the screen currently is in the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The provider/action menu.
    Idle,
    /// A `StartLoopback` was issued; awaiting `LoopbackStarted`.
    Starting,
    /// The loopback listener is live; browser round-trip in progress.
    Waiting,
    /// A focused single-line token input.
    TokenEntry,
    /// A captured/pasted token is being verified.
    Verifying,
}

/// The pure login-screen state machine.
pub struct LoginScreen {
    base_url: String,
    provider: Provider,
    phase: Phase,
    url: Option<String>,
    port: Option<u16>,
    input: String,
    error: Option<String>,
    flash: Option<String>,
    frame: usize,
    outcome: Option<LoginOutcome>,
}

impl LoginScreen {
    /// A fresh screen for `base_url`, starting on the provider menu.
    pub fn new(base_url: impl Into<String>) -> Self {
        LoginScreen {
            base_url: base_url.into(),
            provider: Provider::default(),
            phase: Phase::Idle,
            url: None,
            port: None,
            input: String::new(),
            error: None,
            flash: None,
            frame: 0,
            outcome: None,
        }
    }

    /// The terminal outcome, once the screen has reached one.
    pub fn outcome(&self) -> Option<LoginOutcome> {
        self.outcome.clone()
    }

    /// The currently-selected provider.
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// Advance the spinner (called on the pre-app loop tick).
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

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

    fn spinner(&self) -> &'static str {
        SPINNER[self.frame % SPINNER.len()]
    }

    /// Render the centered login panel.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = centered_rect(64, 17, f.area());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                " login ",
                Style::default().add_modifier(Modifier::DIM),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        for row in crate::ui::LOGO {
            lines.push(Line::from(Span::styled(
                row,
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        lines.push(Line::from(Span::styled(
            format!("backend {}", self.base_url),
            Style::default().add_modifier(Modifier::DIM),
        )));
        lines.push(Line::from(vec![
            Span::styled("provider ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(
                self.provider.as_str(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        match self.phase {
            Phase::Idle => {
                lines.push(Line::from(Span::styled(
                    "Enter/o  log in via browser",
                    Style::default(),
                )));
                lines.push(Line::from("←/→ or p  change provider"));
                lines.push(Line::from("t  paste a token"));
                lines.push(Line::from("m  continue offline (mock)"));
                lines.push(Line::from("q  quit"));
            }
            Phase::Starting => {
                lines.push(Line::from(format!("{} starting loopback…", self.spinner())));
            }
            Phase::Waiting => {
                if let Some(url) = &self.url {
                    lines.push(Line::from(Span::styled(
                        url.clone(),
                        Style::default().fg(Color::Blue),
                    )));
                    lines.push(Line::from(""));
                }
                let port = self.port.map(|p| p.to_string()).unwrap_or_default();
                lines.push(Line::from(format!(
                    "{} waiting for browser callback on 127.0.0.1:{port} …",
                    self.spinner()
                )));
                lines.push(Line::from(Span::styled(
                    "Esc  cancel",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Phase::TokenEntry => {
                lines.push(Line::from(Span::styled(
                    "Paste a JWT or 64-hex login token, Enter to submit:",
                    Style::default().add_modifier(Modifier::DIM),
                )));
                let shown = token_display(&self.input, 56);
                lines.push(Line::from(vec![
                    Span::raw("> "),
                    Span::styled(shown, Style::default().add_modifier(Modifier::DIM)),
                ]));
                lines.push(Line::from(Span::styled(
                    "Esc  cancel",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Phase::Verifying => {
                lines.push(Line::from(format!("{} verifying…", self.spinner())));
            }
        }

        if let Some(flash) = &self.flash {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                flash.clone(),
                Style::default().fg(Color::Green),
            )));
        }
        if let Some(err) = &self.error {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("error: {err}"),
                Style::default().fg(Color::Red),
            )));
            lines.push(Line::from(Span::styled(
                "press Enter/o to retry",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }

        f.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: false }),
            inner,
        );
    }
}

/// Dim/truncate a token for display (no masking): show a leading window with an
/// ellipsis when it overflows `width`.
fn token_display(token: &str, width: usize) -> String {
    if token.is_empty() {
        return String::new();
    }
    let count = token.chars().count();
    if count <= width {
        token.to_string()
    } else {
        let take = width.saturating_sub(1);
        let mut out: String = token.chars().take(take).collect();
        out.push('…');
        out
    }
}

/// A `w`×`h` rectangle centered in `area` (clamped to the area's size).
fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(h)) / 2),
            Constraint::Length(h),
            Constraint::Min(0),
        ])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(w)) / 2),
            Constraint::Length(w),
            Constraint::Min(0),
        ])
        .split(rows[1]);
    cols[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn render(screen: &mut LoginScreen) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| screen.draw(f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        buf.content().iter().map(|c| c.symbol()).collect::<String>()
    }

    #[test]
    fn renders_branding_and_backend() {
        let mut s = LoginScreen::new("http://localhost:5000");
        let out = render(&mut s);
        assert!(out.contains("▛▛▌█▌▛▌▌▌▐ ▐ ▀▌"), "logo: {out}");
        assert!(out.contains("localhost:5000"), "base url: {out}");
        assert!(out.contains("google"), "default provider: {out}");
    }

    #[test]
    fn provider_cycles_with_arrows_and_p() {
        let mut s = LoginScreen::new("x");
        assert_eq!(s.provider(), Provider::Google);
        assert!(s.handle_key(key(KeyCode::Right)).is_none());
        assert_eq!(s.provider(), Provider::Github);
        s.handle_key(key(KeyCode::Char('p')));
        assert_eq!(s.provider(), Provider::Twitter);
        s.handle_key(key(KeyCode::Left));
        assert_eq!(s.provider(), Provider::Github);
        s.handle_key(key(KeyCode::Left));
        assert_eq!(s.provider(), Provider::Google);
        // Wrap backwards past the start.
        s.handle_key(key(KeyCode::Left));
        assert_eq!(s.provider(), Provider::Discord);
    }

    #[test]
    fn enter_and_o_start_loopback() {
        let mut s = LoginScreen::new("http://b");
        let cmd = s.handle_key(key(KeyCode::Enter));
        assert_eq!(
            cmd,
            Some(LoginCmd::StartLoopback {
                base_url: "http://b".into(),
                provider: Provider::Google,
            })
        );
        // 'o' also starts (from Idle after a cancel).
        s.apply(LoginEvent::CallbackError("cancelled".into()));
        let cmd = s.handle_key(key(KeyCode::Char('o')));
        assert!(matches!(cmd, Some(LoginCmd::StartLoopback { .. })));
    }

    #[test]
    fn esc_while_waiting_cancels_loopback() {
        let mut s = LoginScreen::new("b");
        s.handle_key(key(KeyCode::Enter));
        s.apply(LoginEvent::LoopbackStarted {
            url: "http://b/auth/google/login".into(),
            port: 40404,
        });
        let out = render(&mut s);
        assert!(
            out.contains("waiting for browser callback"),
            "waiting: {out}"
        );
        assert!(out.contains("40404"), "port: {out}");
        assert!(out.contains("http://b/auth/google/login"), "url: {out}");
        let cmd = s.handle_key(key(KeyCode::Esc));
        assert_eq!(cmd, Some(LoginCmd::CancelLoopback));
    }

    #[test]
    fn token_entry_edits_and_submits() {
        let mut s = LoginScreen::new("b");
        assert!(s.handle_key(key(KeyCode::Char('t'))).is_none());
        for c in "abc".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        s.handle_key(key(KeyCode::Backspace));
        let out = render(&mut s);
        assert!(out.contains("ab"), "input echoed: {out}");
        let cmd = s.handle_key(key(KeyCode::Enter));
        assert_eq!(cmd, Some(LoginCmd::SubmitToken("ab".into())));
        // Empty submit is refused with an error, no command.
        let mut s2 = LoginScreen::new("b");
        s2.handle_key(key(KeyCode::Char('t')));
        assert!(s2.handle_key(key(KeyCode::Enter)).is_none());
        assert!(render(&mut s2).contains("enter a token"));
    }

    #[test]
    fn m_and_q_yield_outcomes() {
        let mut m = LoginScreen::new("b");
        m.handle_key(key(KeyCode::Char('m')));
        assert_eq!(m.outcome(), Some(LoginOutcome::Mock));

        let mut q = LoginScreen::new("b");
        q.handle_key(key(KeyCode::Char('q')));
        assert_eq!(q.outcome(), Some(LoginOutcome::Quit));

        let mut c = LoginScreen::new("b");
        c.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(c.outcome(), Some(LoginOutcome::Quit));
    }

    #[test]
    fn verified_sets_token_outcome_and_flashes() {
        let mut s = LoginScreen::new("b");
        s.apply(LoginEvent::CallbackToken("jwt".into()));
        assert!(render(&mut s).contains("verifying"));
        s.apply(LoginEvent::Verified {
            jwt: "jwt-1".into(),
            who: "Logged in as a@b.c".into(),
        });
        assert_eq!(s.outcome(), Some(LoginOutcome::Token("jwt-1".into())));
        assert!(render(&mut s).contains("Logged in as a@b.c"));
    }

    #[test]
    fn errors_render_inline_and_keep_screen_usable() {
        let mut s = LoginScreen::new("b");
        s.apply(LoginEvent::VerifyFailed("bad token".into()));
        let out = render(&mut s);
        assert!(out.contains("bad token"), "error: {out}");
        assert!(out.contains("retry"), "retry hint: {out}");
        // Still usable: can start over.
        assert!(matches!(
            s.handle_key(key(KeyCode::Enter)),
            Some(LoginCmd::StartLoopback { .. })
        ));

        let mut s2 = LoginScreen::new("b");
        s2.apply(LoginEvent::CallbackError("state mismatch timeout".into()));
        assert!(render(&mut s2).contains("state mismatch timeout"));
    }

    #[test]
    fn tick_advances_spinner_without_panic() {
        let mut s = LoginScreen::new("b");
        s.handle_key(key(KeyCode::Enter));
        s.apply(LoginEvent::LoopbackStarted {
            url: "u".into(),
            port: 1,
        });
        for _ in 0..25 {
            s.tick();
        }
        let _ = render(&mut s);
    }

    #[test]
    fn token_display_truncates() {
        assert_eq!(token_display("", 4), "");
        assert_eq!(token_display("abc", 4), "abc");
        assert_eq!(token_display("abcdef", 4), "abc…");
    }
}
