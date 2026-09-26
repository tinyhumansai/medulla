//! The "this account needs a subscription" screen.
//!
//! Drawn when [`medulla::access::decide`] refuses the signed-in account, in
//! place of the app. A pure state machine like [`super::login`]: it renders,
//! takes keys, and reaches an outcome; re-asking the backend, opening a browser,
//! clearing the stored session and leaving the process are the caller's jobs.
//!
//! Deliberately not a nag that can be dismissed into the app. The gate exists to
//! say one thing — this account needs a plan — and a screen that could be
//! skipped would say it while meaning nothing.
//!
//! # Why it offers more than "Subscribe" and "Quit"
//!
//! The two things an operator actually does here both used to dead-end. Buying a
//! plan in the browser left the terminal showing a refusal it had no way to
//! re-check, so the only way back in was to quit and relaunch — which looks, to
//! somebody who has just paid, like the purchase did not work. And an operator
//! signed in to the wrong account (a personal login where the subscribed work
//! account was meant) had no way to reach a login screen, because the login
//! screen only runs when there is *no* session at all.
//!
//! So the menu carries [`Item::CheckAgain`] and [`Item::SignOut`] as well. They
//! do not weaken the gate: the first re-asks the backend and is admitted only if
//! the backend now says yes, and the second ends the session rather than
//! bypassing the check on it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use medulla::access::{Denial, TRIAL_DAYS};

use super::theme::Theme;

/// Where the operator is sent to subscribe.
pub const PRICING_URL: &str = "https://tinyhumans.ai/pricing";

/// The rule, for an operator who has never been told it.
///
/// Shown on every refusal regardless of the reason: somebody who has just been
/// refused should not have to infer what would fix it. Built from
/// [`medulla::access::TRIAL`] rather than written out, so the sentence cannot
/// promise a window the policy does not grant.
///
/// Two short lines rather than one long one because the panel sizes itself to
/// its longest line: a single sentence would either widen the panel past the
/// logo or wrap mid-rule on a narrow terminal.
pub fn requirement_lines() -> [String; 2] {
    [
        "Medulla requires a Basic subscription or higher.".to_string(),
        format!("New accounts can use it free for their first {TRIAL_DAYS} days."),
    ]
}

/// What the screen resolved to.
///
/// Each variant is something only the caller can carry out — see the module
/// docs. The screen sets one and stops; the caller acts and, for
/// [`UpgradeOutcome::CheckAgain`], hands the answer back through
/// [`UpgradeScreen::rechecked`] or [`UpgradeScreen::recheck_failed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeOutcome {
    /// Leave the process.
    Quit,
    /// Ask the backend again whether this account may use Medulla.
    CheckAgain,
    /// Forget the stored session, so the next launch signs in fresh.
    SignOut,
}

/// An async action the caller runs on the screen's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeCmd {
    /// Open `url` in the platform browser. Fire-and-forget.
    OpenUrl(String),
}

/// One row of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    /// Open the pricing page. Does not leave the screen.
    Subscribe,
    /// Re-ask the backend, for somebody who has just subscribed.
    CheckAgain,
    /// Sign out, for somebody who is on the wrong account.
    SignOut,
    /// Leave with [`UpgradeOutcome::Quit`].
    Quit,
}

impl Item {
    fn label(self) -> &'static str {
        match self {
            Item::Subscribe => "Subscribe",
            Item::CheckAgain => "Check again",
            Item::SignOut => "Sign out",
            Item::Quit => "Quit",
        }
    }
}

const MENU: [Item; 4] = [Item::Subscribe, Item::CheckAgain, Item::SignOut, Item::Quit];

/// The subscription gate's screen state.
pub struct UpgradeScreen {
    /// The account this refers to, as `describe_me` phrased it.
    who: String,
    /// Why the account was refused, which decides what the panel says.
    denial: Denial,
    menu_index: usize,
    flash: Option<Flash>,
    outcome: Option<UpgradeOutcome>,
}

/// A one-line status under the menu, and how to colour it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Flash {
    text: String,
    /// Whether this reports something going right. Colours it green rather than
    /// yellow — a refusal that reads as a success is worse than no colour.
    good: bool,
}

impl UpgradeScreen {
    /// A screen for the account described by `who`, refused for `denial`.
    pub fn new(who: impl Into<String>, denial: Denial) -> Self {
        UpgradeScreen {
            who: who.into(),
            denial,
            menu_index: 0,
            flash: None,
            outcome: None,
        }
    }

    /// The headline and the line under it, for this denial.
    ///
    /// [`Denial::Undetermined`] does not mention the trial at all. The operator
    /// may be a paying customer whose plan we simply failed to look up, and
    /// telling them their free trial is over would be both wrong and insulting.
    fn message(&self) -> (&'static str, &'static str) {
        match self.denial {
            Denial::TrialExpired => (
                "Your free trial has ended.",
                "Subscribe to Basic or Pro to carry on where you left off.",
            ),
            Denial::Undetermined => (
                "Your subscription could not be confirmed.",
                "This is usually us, not you. Wait a moment, then check again.",
            ),
        }
    }

    /// The outcome, once the screen has reached one.
    pub fn outcome(&self) -> Option<UpgradeOutcome> {
        self.outcome.clone()
    }

    /// Take the outcome, leaving the screen ready to run on.
    ///
    /// [`UpgradeOutcome::CheckAgain`] is the reason this exists: the caller acts
    /// on it and, when the answer is still no, the same screen keeps running. A
    /// caller that peeked with [`UpgradeScreen::outcome`] instead would re-check
    /// forever on the outcome it never cleared.
    pub fn take_outcome(&mut self) -> Option<UpgradeOutcome> {
        self.outcome.take()
    }

    /// Report a re-check that reached the backend and was still refused.
    ///
    /// `who` is re-read from the fresh `/auth/me` rather than kept, so an
    /// account that changed under the screen is named correctly.
    pub fn rechecked(&mut self, who: impl Into<String>, denial: Denial) {
        self.who = who.into();
        self.denial = denial;
        self.flash = Some(Flash {
            text: match denial {
                Denial::TrialExpired => "Checked — this account still has no subscription.".into(),
                Denial::Undetermined => {
                    "Checked — your subscription still could not be read.".into()
                }
            },
            good: false,
        });
    }

    /// Report a re-check that could not reach the backend at all.
    ///
    /// Distinct from [`UpgradeScreen::rechecked`]: nothing was learned about the
    /// account, so the panel's headline must not change on the strength of it.
    pub fn recheck_failed(&mut self, why: &str) {
        self.flash = Some(Flash {
            text: format!("Could not reach the backend: {why}"),
            good: false,
        });
    }

    /// Handle one key, optionally emitting a command.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<UpgradeCmd> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.outcome = Some(UpgradeOutcome::Quit);
            return None;
        }
        match key.code {
            KeyCode::Up => {
                self.menu_index = (self.menu_index + MENU.len() - 1) % MENU.len();
                None
            }
            KeyCode::Down => {
                self.menu_index = (self.menu_index + 1) % MENU.len();
                None
            }
            // Esc leaves. There is nothing to back out to: the app behind this
            // screen is the thing the account may not use.
            KeyCode::Esc => {
                self.outcome = Some(UpgradeOutcome::Quit);
                None
            }
            // A shortcut for the one action an operator repeats: subscribe in
            // the browser, come back, check, wait, check again.
            KeyCode::Char('r') => {
                self.begin_recheck();
                None
            }
            KeyCode::Enter => match MENU[self.menu_index.min(MENU.len() - 1)] {
                Item::Subscribe => {
                    self.flash = Some(Flash {
                        text: format!(
                            "Opened {PRICING_URL} — subscribe there, then choose \"Check again\"."
                        ),
                        good: true,
                    });
                    Some(UpgradeCmd::OpenUrl(PRICING_URL.to_string()))
                }
                Item::CheckAgain => {
                    self.begin_recheck();
                    None
                }
                Item::SignOut => {
                    self.outcome = Some(UpgradeOutcome::SignOut);
                    None
                }
                Item::Quit => {
                    self.outcome = Some(UpgradeOutcome::Quit);
                    None
                }
            },
            _ => None,
        }
    }

    /// Ask for a re-check, and say so before the caller blocks on the network.
    ///
    /// The flash is set here rather than by the caller because the frame drawn
    /// between the keypress and the answer is this one: without it the screen
    /// sits unchanged for the length of an HTTP round trip and reads as frozen.
    fn begin_recheck(&mut self) {
        self.flash = Some(Flash {
            text: "Checking your subscription…".to_string(),
            good: true,
        });
        self.outcome = Some(UpgradeOutcome::CheckAgain);
    }

    /// Render the centered panel.
    pub fn draw(&mut self, f: &mut Frame) {
        let theme = Theme::default();
        let mut head: Vec<Line> = Vec::new();
        for row in super::LOGO {
            head.push(Line::from(Span::styled(
                row,
                Style::default()
                    .fg(theme.primary)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        head.push(Line::from(""));
        head.push(Line::from(Span::styled(
            self.who.clone(),
            Style::default().add_modifier(Modifier::DIM),
        )));
        head.push(Line::from(""));
        let (headline, detail) = self.message();
        head.push(Line::from(Span::styled(
            headline,
            Style::default().add_modifier(Modifier::BOLD),
        )));
        head.push(Line::from(detail));
        head.push(Line::from(""));
        // The rule itself, so the operator is told what access needs rather than
        // left to work it out from a refusal.
        for rule in requirement_lines() {
            head.push(Line::from(rule));
        }
        head.push(Line::from(""));
        head.push(Line::from(Span::styled(
            format!("  {PRICING_URL}"),
            Style::default().fg(Color::Blue),
        )));

        // Restates what the menu rows already offer, so on a tight terminal it
        // is the first thing dropped — see below.
        let hints: Vec<Line> = vec![
            Line::from(Span::styled(
                "Already subscribed? Choose \"Check again\" — no restart needed.",
                Style::default().add_modifier(Modifier::DIM),
            )),
            Line::from(Span::styled(
                "On the wrong account? Choose \"Sign out\" and start Medulla again.",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];

        let mut tail: Vec<Line> = Vec::new();
        for (i, item) in MENU.iter().enumerate() {
            let selected = i == self.menu_index;
            let style = if selected {
                Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            tail.push(Line::from(Span::styled(
                format!("{} {:<32}", if selected { "▸" } else { " " }, item.label()),
                style,
            )));
        }
        tail.push(Line::from(""));
        tail.push(Line::from(Span::styled(
            "↑↓ choose · Enter select · r check again · Esc quit",
            Style::default().add_modifier(Modifier::DIM),
        )));

        let mut flash_lines: Vec<Line> = Vec::new();
        if let Some(flash) = &self.flash {
            flash_lines.push(Line::from(""));
            flash_lines.push(Line::from(Span::styled(
                flash.text.clone(),
                Style::default().fg(if flash.good {
                    Color::Green
                } else {
                    Color::Yellow
                }),
            )));
        }

        let screen = f.area();

        // Whether the hint lines fit depends on the terminal, not on their own
        // content, so width is sized from the full candidate set — including
        // them — regardless of whether they end up drawn: the panel must not
        // visibly resize depending on how tall the terminal happens to be.
        let assemble = |include_hints: bool| -> Vec<Line<'static>> {
            let mut all = head.clone();
            all.push(Line::from(""));
            if include_hints {
                all.extend(hints.clone());
                all.push(Line::from(""));
            }
            all.extend(tail.clone());
            all.extend(flash_lines.clone());
            all
        };
        let full = assemble(true);
        let width = super::layout::panel_width(&full, screen.width);

        // On a short terminal, the flash line — the entire point of a
        // re-check — is the thing that must stay visible: `panel_height`
        // clamps to the screen and the paragraph never scrolls, so whatever
        // does not fit is simply cut off the bottom. The two "Already
        // subscribed?" / "On the wrong account?" lines restate the menu rows
        // right below them, so they are the first content dropped when
        // everything does not fit — never the flash.
        let available = screen.height.saturating_sub(2).max(1);
        let needed = super::layout::panel_height(&full, width, u16::MAX).saturating_sub(2);
        let lines = if needed <= available {
            full
        } else {
            assemble(false)
        };

        let height = super::layout::panel_height(&lines, width, screen.height);
        let area = super::layout::centered_fixed(width, height, screen);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.primary))
            .title(Span::styled(
                " subscribe ",
                Style::default().add_modifier(Modifier::DIM),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: false }),
            inner,
        );
    }
}

#[cfg(test)]
#[path = "upgrade_tests.rs"]
mod tests;
