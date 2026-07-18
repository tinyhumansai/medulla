//! The first-run worker onboarding screen: a pure state machine the onboarding
//! pre-run loop drives before a worker (daemon/wrapper) starts serving.
//!
//! It mirrors [`crate::ui::login`]: [`OnboardingScreen::handle_key`] turns keys
//! into [`OnboardingCmd`]s, [`OnboardingScreen::apply`] folds
//! [`OnboardingEvent`]s from async work back into state, and
//! [`OnboardingScreen::draw`] renders the centered panel. The loop reads
//! [`OnboardingScreen::outcome`] to know when to stop.
//!
//! Three steps: NAME (an input prefilled with the default worker name) →
//! CONNECTION (create/load the identity, then prompt for the OpenHuman owner) →
//! CONFIRM (a summary panel). The heavy work — minting the identity, sending the
//! announce DM, writing the profile — lives in [`crate::onboarding`]; this module
//! only owns state and rendering.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::app::SPINNER;

/// The terminal outcome of the onboarding screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingOutcome {
    /// Registration confirmed: persist the profile with this name and owner.
    Register { name: String, owner: Option<String> },
    /// Abort without writing anything (q / Ctrl-C).
    Abort,
}

/// An async action the pre-run loop must run on the screen's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingCmd {
    /// Create/load the tiny.place identity for the chosen worker name.
    LoadIdentity { name: String },
}

/// An event fed back from spawned async work into [`OnboardingScreen::apply`].
#[derive(Debug, Clone)]
pub enum OnboardingEvent {
    /// The identity is ready: its wallet `address` and optional `@handle`.
    IdentityReady {
        address: String,
        handle: Option<String>,
    },
    /// Identity bootstrap failed.
    IdentityFailed(String),
}

/// Which step the flow is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Naming the worker.
    Name,
    /// Waiting for the identity to load (spinner).
    Connecting,
    /// Identity ready; entering/confirming the owner.
    Owner,
    /// The summary panel.
    Confirm,
}

/// The pure onboarding-screen state machine.
pub struct OnboardingScreen {
    endpoint: String,
    step: Step,
    name: String,
    owner: String,
    address: Option<String>,
    handle: Option<String>,
    error: Option<String>,
    flash: Option<String>,
    frame: usize,
    outcome: Option<OnboardingOutcome>,
}

impl OnboardingScreen {
    /// A fresh screen prefilled with `default_name`, an optional `env_owner` (from
    /// the owner env chain), and the resolved `endpoint` for the summary.
    pub fn new(
        default_name: impl Into<String>,
        env_owner: Option<String>,
        endpoint: impl Into<String>,
    ) -> Self {
        OnboardingScreen {
            endpoint: endpoint.into(),
            step: Step::Name,
            name: default_name.into(),
            owner: env_owner.unwrap_or_default(),
            address: None,
            handle: None,
            error: None,
            flash: None,
            frame: 0,
            outcome: None,
        }
    }

    /// The terminal outcome, once reached.
    pub fn outcome(&self) -> Option<OnboardingOutcome> {
        self.outcome.clone()
    }

    /// The current worker name draft.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Advance the spinner (called on the pre-run loop tick).
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn spinner(&self) -> &'static str {
        SPINNER[self.frame % SPINNER.len()]
    }

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

    /// Render the centered onboarding panel.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = centered_rect(66, 18, f.area());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                " worker setup ",
                Style::default().add_modifier(Modifier::DIM),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "MEDULLA WORKER",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "first-run registration",
            Style::default().add_modifier(Modifier::DIM),
        )));
        lines.push(Line::from(""));

        match self.step {
            Step::Name => {
                lines.push(Line::from(Span::styled(
                    "Step 1/3 · name this worker",
                    Style::default().fg(Color::Magenta),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::raw("name > "),
                    Span::styled(
                        self.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    "Enter to accept",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Step::Connecting => {
                lines.push(Line::from(Span::styled(
                    "Step 2/3 · connection",
                    Style::default().fg(Color::Magenta),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "{} setting up the tiny.place identity…",
                    self.spinner()
                )));
            }
            Step::Owner => {
                lines.push(Line::from(Span::styled(
                    "Step 2/3 · connection",
                    Style::default().fg(Color::Magenta),
                )));
                lines.push(Line::from(""));
                if let Some(address) = &self.address {
                    lines.push(Line::from(vec![
                        Span::styled("address ", Style::default().add_modifier(Modifier::DIM)),
                        Span::styled(address.clone(), Style::default().fg(Color::Green)),
                    ]));
                }
                if let Some(handle) = &self.handle {
                    lines.push(Line::from(vec![
                        Span::styled("handle  ", Style::default().add_modifier(Modifier::DIM)),
                        Span::styled(handle.clone(), Style::default().fg(Color::Cyan)),
                    ]));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "OpenHuman owner (@handle or address):",
                    Style::default().add_modifier(Modifier::DIM),
                )));
                lines.push(Line::from(vec![
                    Span::raw("owner > "),
                    Span::styled(
                        self.owner.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    "Enter to save · Esc to skip",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Step::Confirm => {
                lines.push(Line::from(Span::styled(
                    "Step 3/3 · confirm",
                    Style::default().fg(Color::Magenta),
                )));
                lines.push(Line::from(""));
                lines.push(summary_line("name", &self.name));
                lines.push(summary_line(
                    "address",
                    self.address.as_deref().unwrap_or("(none)"),
                ));
                lines.push(summary_line(
                    "owner",
                    if self.owner.trim().is_empty() {
                        "(none — set later)"
                    } else {
                        self.owner.trim()
                    },
                ));
                lines.push(summary_line("endpoint", &self.endpoint));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Enter to finish · Esc to edit owner · q to abort",
                    Style::default().add_modifier(Modifier::DIM),
                )));
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
        }

        f.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: false }),
            inner,
        );
    }
}

fn summary_line<'a>(label: &'a str, value: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{label:<9}"),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
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

    fn render(screen: &mut OnboardingScreen) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
        terminal.draw(|f| screen.draw(f)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn prefills_name_and_walks_to_identity() {
        let mut s = OnboardingScreen::new("ada@box/10.0.0.4", None, "https://api.tiny.place");
        let out = render(&mut s);
        assert!(out.contains("MEDULLA WORKER"), "branding: {out}");
        assert!(out.contains("ada@box/10.0.0.4"), "prefilled name: {out}");
        assert!(out.contains("Step 1/3"), "step label: {out}");

        let cmd = s.handle_key(key(KeyCode::Enter));
        assert_eq!(
            cmd,
            Some(OnboardingCmd::LoadIdentity {
                name: "ada@box/10.0.0.4".into()
            })
        );
        assert!(render(&mut s).contains("setting up the tiny.place identity"));
    }

    #[test]
    fn name_is_editable() {
        let mut s = OnboardingScreen::new("abc", None, "e");
        s.handle_key(key(KeyCode::Backspace));
        s.handle_key(key(KeyCode::Char('x')));
        assert_eq!(s.name(), "abx");
        assert!(render(&mut s).contains("abx"));
    }

    #[test]
    fn empty_name_is_refused() {
        let mut s = OnboardingScreen::new("", None, "e");
        assert!(s.handle_key(key(KeyCode::Enter)).is_none());
        assert!(render(&mut s).contains("enter a worker name"));
    }

    #[test]
    fn identity_ready_advances_and_shows_address_and_handle() {
        let mut s = OnboardingScreen::new("w", None, "e");
        s.handle_key(key(KeyCode::Enter));
        s.apply(OnboardingEvent::IdentityReady {
            address: "AgentAddr111".into(),
            handle: Some("@ada".into()),
        });
        let out = render(&mut s);
        assert!(out.contains("AgentAddr111"), "address: {out}");
        assert!(out.contains("@ada"), "handle: {out}");
        assert!(out.contains("OpenHuman owner"), "owner prompt: {out}");
    }

    #[test]
    fn env_owner_prefills_owner_field() {
        let mut s = OnboardingScreen::new("w", Some("@overseer".into()), "e");
        s.handle_key(key(KeyCode::Enter));
        s.apply(OnboardingEvent::IdentityReady {
            address: "A".into(),
            handle: None,
        });
        assert!(render(&mut s).contains("@overseer"), "env owner prefilled");
    }

    #[test]
    fn owner_entered_then_confirmed_registers() {
        let mut s = OnboardingScreen::new("w", None, "e");
        s.handle_key(key(KeyCode::Enter));
        s.apply(OnboardingEvent::IdentityReady {
            address: "A".into(),
            handle: None,
        });
        for c in "@boss".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        // Enter → confirm step.
        assert!(s.handle_key(key(KeyCode::Enter)).is_none());
        let out = render(&mut s);
        assert!(out.contains("Step 3/3"), "confirm step: {out}");
        assert!(out.contains("@boss"), "owner in summary: {out}");
        // Enter → register.
        s.handle_key(key(KeyCode::Enter));
        assert_eq!(
            s.outcome(),
            Some(OnboardingOutcome::Register {
                name: "w".into(),
                owner: Some("@boss".into())
            })
        );
    }

    #[test]
    fn esc_skips_owner_with_a_note() {
        let mut s = OnboardingScreen::new("w", Some("@pre".into()), "e");
        s.handle_key(key(KeyCode::Enter));
        s.apply(OnboardingEvent::IdentityReady {
            address: "A".into(),
            handle: None,
        });
        // Esc skips — forgetting the prefilled owner.
        s.handle_key(key(KeyCode::Esc));
        let out = render(&mut s);
        assert!(out.contains("set later"), "skip note: {out}");
        assert!(out.contains("(none"), "owner none in summary: {out}");
        s.handle_key(key(KeyCode::Enter));
        assert_eq!(
            s.outcome(),
            Some(OnboardingOutcome::Register {
                name: "w".into(),
                owner: None
            })
        );
    }

    #[test]
    fn confirm_esc_returns_to_owner_editing() {
        let mut s = OnboardingScreen::new("w", None, "e");
        s.handle_key(key(KeyCode::Enter));
        s.apply(OnboardingEvent::IdentityReady {
            address: "A".into(),
            handle: None,
        });
        s.handle_key(key(KeyCode::Enter)); // → confirm
        s.handle_key(key(KeyCode::Esc)); // → back to owner
        assert!(render(&mut s).contains("OpenHuman owner"));
    }

    #[test]
    fn q_and_ctrl_c_abort() {
        let mut s = OnboardingScreen::new("w", None, "e");
        s.handle_key(key(KeyCode::Enter));
        s.apply(OnboardingEvent::IdentityReady {
            address: "A".into(),
            handle: None,
        });
        s.handle_key(key(KeyCode::Enter)); // → confirm
        s.handle_key(key(KeyCode::Char('q')));
        assert_eq!(s.outcome(), Some(OnboardingOutcome::Abort));

        let mut c = OnboardingScreen::new("w", None, "e");
        c.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(c.outcome(), Some(OnboardingOutcome::Abort));
    }

    #[test]
    fn identity_failed_returns_to_name_with_error() {
        let mut s = OnboardingScreen::new("w", None, "e");
        s.handle_key(key(KeyCode::Enter));
        s.apply(OnboardingEvent::IdentityFailed("keygen boom".into()));
        let out = render(&mut s);
        assert!(out.contains("keygen boom"), "error shown: {out}");
        assert!(out.contains("Step 1/3"), "back to name: {out}");
        // Still usable: re-submit.
        assert!(matches!(
            s.handle_key(key(KeyCode::Enter)),
            Some(OnboardingCmd::LoadIdentity { .. })
        ));
    }

    #[test]
    fn tick_advances_spinner_without_panic() {
        let mut s = OnboardingScreen::new("w", None, "e");
        s.handle_key(key(KeyCode::Enter));
        for _ in 0..20 {
            s.tick();
        }
        let _ = render(&mut s);
    }
}
