//! Guided entry point for connecting another *machine*.
//!
//! Remote only, and deliberately. A "local host" used to be a kind offered
//! here — a harness plus a directory on this machine — but that is exactly what
//! an agent is now, and it is declared where the agents are: `n` on a local host
//! row in the Hosts tab. Offering both meant two flows that wrote the same thing
//! and disagreed about what to call it.
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
    /// Draw the Add Host wizard: what to run over there, then what to paste here.
    ///
    /// Both steps stay on screen so the shape of the task is visible from the
    /// first keypress. Each copy it asks for runs in the *easy* direction — step
    /// one is copied from this local terminal, step two is put on this
    /// terminal's clipboard by the remote worker over OSC 52.
    pub(super) fn draw_add_host(&self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let live = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);

        let header =
            |n: usize, title: &str| TLine::from(Span::styled(format!("{n} · {title}"), live));

        let mut lines = vec![
            TLine::from("Connect another machine for the orchestrator to delegate to."),
            TLine::from(Span::styled(
                "To add an agent on this device, press n on its host row in the tree.",
                dim,
            )),
            TLine::from(""),
        ];

        lines.push(header(1, "On the machine you want to add"));
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            format!("    {REMOTE_JOIN_COMMAND}"),
            Style::default().fg(Color::Green),
        )));
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "    Press c to copy the installer, then paste it into an SSH session.",
            dim,
        )));
        lines.push(TLine::from(Span::styled(
            "    Provision the host-link identity before starting `medulla daemon`.",
            dim,
        )));
        lines.push(TLine::from(""));
        lines.push(header(2, "Back here"));
        lines.push(TLine::from(""));
        lines.push(TLine::from(
            "    The worker prints its address and puts it on your clipboard,",
        ));
        lines.push(TLine::from(
            "    even over SSH. Press Enter and paste it, with an optional label.",
        ));
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "    Example: 7Kx…9fQ Primary build machine",
            dim,
        )));
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "Enter paste the address · c copy the install line · Esc back to the menu",
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
