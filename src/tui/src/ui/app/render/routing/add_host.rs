//! Guided entry point for connecting another host.
//!
//! The page is written as two steps because pairing is genuinely two steps, and
//! because each copy it asks for runs in the *easy* direction. Step one is
//! copied here — a local terminal, where copying is trivial — and pasted into
//! the remote shell. Step two is produced on the remote host, but the worker
//! hands it to this terminal's clipboard over OSC 52 rather than asking anyone
//! to select base58 out of an SSH scrollback (see
//! [`medulla::daemon::pairing`]).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::daemon::pairing::REMOTE_JOIN_COMMAND;

use super::super::super::types::App;

impl App {
    /// The harnesses a new local host can be pointed at.
    ///
    /// What this machine actually has, not what the protocol knows about: a
    /// picker offering a CLI that is not installed produces a host that accepts
    /// work and fails every task.
    pub(in crate::ui::app) fn add_host_providers(
        &self,
    ) -> Vec<medulla::tinyplace::HarnessProvider> {
        let env: std::collections::HashMap<String, String> = std::env::vars().collect();
        let detected = medulla::daemon::providers::detect_providers(&env, None, None);
        if detected.is_empty() {
            vec![medulla::tinyplace::HarnessProvider::Claude]
        } else {
            detected
        }
    }

    /// The kind currently under the cursor.
    pub(in crate::ui::app) fn add_host_selected_kind(
        &self,
    ) -> super::super::super::types::AddHostKind {
        use super::super::super::types::AddHostKind;
        AddHostKind::ALL[self.add_host_kind.min(AddHostKind::ALL.len() - 1)]
    }

    /// Draw the Add Host wizard: one step live at a time, the rest shown but
    /// inert.
    ///
    /// Every step stays on screen so the shape of the task is visible from the
    /// first keypress — but only the live one carries a cursor and full colour.
    /// Rendering two identical-looking lists at once was the problem this
    /// fixes: the arrows drive exactly one of them and nothing said which.
    pub(super) fn draw_add_host(&self, f: &mut Frame, area: Rect) {
        use super::super::super::types::AddHostKind;

        let dim = Style::default().add_modifier(Modifier::DIM);
        let live = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let done = Style::default().fg(Color::Green);
        let picked = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);

        let kind = self.add_host_selected_kind();
        let on_kind = !self.add_host_kind_chosen;

        // A step header: live when it is the one taking keys, done when it is
        // behind the cursor, dim when it is still ahead.
        let header = |n: usize, title: &str, state: StepState| {
            let style = match state {
                StepState::Live => live,
                StepState::Done => done,
                StepState::Ahead => dim,
            };
            let mark = match state {
                StepState::Done => "✓",
                _ => "·",
            };
            TLine::from(Span::styled(format!("{n} {mark} {title}"), style))
        };

        let mut lines = vec![
            TLine::from("Add a host for the orchestrator to delegate to."),
            TLine::from(""),
        ];

        // 1 · kind
        lines.push(header(
            1,
            "What kind of host",
            if on_kind {
                StepState::Live
            } else {
                StepState::Done
            },
        ));
        lines.push(TLine::from(""));
        for (index, option) in AddHostKind::ALL.iter().enumerate() {
            let under_cursor = index == self.add_host_kind.min(AddHostKind::ALL.len() - 1);
            // Once chosen, only the choice remains: the alternatives are noise
            // on a step the operator has already answered.
            if !on_kind && !under_cursor {
                continue;
            }
            let marker = if on_kind && under_cursor {
                "  ▸ "
            } else {
                "    "
            };
            let label_style = match (on_kind, under_cursor) {
                (true, true) => picked,
                (false, true) => done,
                _ => Style::default(),
            };
            lines.push(TLine::from(vec![
                Span::styled(marker, if under_cursor { label_style } else { dim }),
                Span::styled(option.label(), label_style),
                Span::styled(format!("  {}", option.description()), dim),
            ]));
        }
        lines.push(TLine::from(""));

        match kind {
            AddHostKind::Remote => {
                let state = if on_kind {
                    StepState::Ahead
                } else {
                    StepState::Live
                };
                let body = if on_kind { dim } else { Style::default() };
                lines.push(header(2, "On the machine you want to add", state));
                lines.push(TLine::from(""));
                lines.push(TLine::from(Span::styled(
                    format!("    {REMOTE_JOIN_COMMAND}"),
                    if on_kind {
                        dim
                    } else {
                        Style::default().fg(Color::Green)
                    },
                )));
                lines.push(TLine::from(""));
                lines.push(TLine::from(Span::styled(
                    "    Press c to copy that line, then paste it into an SSH session.",
                    dim,
                )));
                lines.push(TLine::from(""));
                lines.push(header(3, "Back here", state));
                lines.push(TLine::from(""));
                lines.push(TLine::from(Span::styled(
                    "    The worker prints its address and puts it on your clipboard,",
                    body,
                )));
                lines.push(TLine::from(Span::styled(
                    "    even over SSH. Press Enter and paste it, with an optional label.",
                    body,
                )));
                lines.push(TLine::from(""));
                lines.push(TLine::from(Span::styled(
                    "    Example: 7Kx…9fQ Primary build machine",
                    dim,
                )));
                lines.push(TLine::from(Span::styled(
                    "    Or run `medulla daemon --handle build-box` and add `@build-box`.",
                    dim,
                )));
            }
            AddHostKind::Local => {
                let state = if on_kind {
                    StepState::Ahead
                } else {
                    StepState::Live
                };
                lines.push(header(2, "Which harness it runs", state));
                lines.push(TLine::from(""));
                let providers = self.add_host_providers();
                for (index, provider) in providers.iter().enumerate() {
                    let under_cursor = index == self.add_host_harness.min(providers.len() - 1);
                    let marker = if !on_kind && under_cursor {
                        "  ▸ "
                    } else {
                        "    "
                    };
                    let style = match (on_kind, under_cursor) {
                        (false, true) => picked,
                        (true, _) => dim,
                        _ => Style::default(),
                    };
                    lines.push(TLine::from(vec![
                        Span::styled(marker, style),
                        Span::styled(provider.as_str().to_string(), style),
                    ]));
                }
                lines.push(TLine::from(""));
                lines.push(header(3, "Where it works", state));
                lines.push(TLine::from(""));
                let body = if on_kind { dim } else { Style::default() };
                lines.push(TLine::from(Span::styled(
                    "    Press Enter and give it a directory. Blank uses this one:",
                    body,
                )));
                lines.push(TLine::from(""));
                lines.push(TLine::from(Span::styled(
                    format!(
                        "    {}",
                        std::env::current_dir()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| ".".to_string())
                    ),
                    dim,
                )));
                lines.push(TLine::from(""));
                lines.push(TLine::from(Span::styled(
                    "    Runs in this process, so its session is watchable and typeable.",
                    dim,
                )));
            }
        }

        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            if on_kind {
                "↑↓ choose a kind · Enter continue · Esc back to the menu"
            } else {
                "↑↓ choose · Enter continue · Esc start over · c copy the install line"
            },
            dim,
        )));

        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: true })
                .block(self.panel("Add Host")),
            area,
        );
    }
}

/// Where a wizard step sits relative to the cursor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StepState {
    /// Behind the cursor: answered.
    Done,
    /// The step taking keys right now.
    Live,
    /// Not reached yet.
    Ahead,
}
