//! The Subscriptions subpage: every usage meter, what it currently reads, and
//! how it is drawn in the sidebar.
//!
//! The page shows a meter's live reading beside its setting rather than only the
//! setting, for the reason the Status line page previews a harness row: the
//! choice being made is about how a number looks, and it cannot be judged
//! without the number. It is also the surface where a meter has room to say what
//! went wrong — the rail has none, and a bare em dash there is a question this
//! page answers.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::config::{MeterId, MeterSource, ResourceDisplay};
use medulla::subscriptions::MeterReading;

use super::super::super::subscriptions::{refresh_label, REFRESH_ROW, SUBSCRIPTION_ROWS};
use super::super::super::types::App;

impl App {
    /// Draw the meter rows, grouped by the subscription they read from.
    pub(super) fn draw_subscriptions(&mut self, f: &mut Frame, area: Rect) {
        let block = self.content_panel("Subscriptions");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let selected = self.subscriptions_index.min(SUBSCRIPTION_ROWS - 1);
        let heading = Style::default().add_modifier(Modifier::BOLD);
        let dim = Style::default().add_modifier(Modifier::DIM);
        let now = medulla::clock::now_millis() / 1000;

        let mut lines: Vec<TLine> = Vec::new();
        let mut selected_line = 0usize;
        let mut source: Option<MeterSource> = None;
        for (row, meter) in MeterId::ALL.into_iter().enumerate() {
            if source != Some(meter.source()) {
                source = Some(meter.source());
                if row > 0 {
                    lines.push(TLine::from(""));
                }
                lines.push(TLine::from(Span::styled(
                    source_title(meter.source()),
                    heading,
                )));
                lines.push(TLine::from(Span::styled(
                    format!("  {}", source_note(meter.source())),
                    dim,
                )));
            }
            if row == selected {
                selected_line = lines.len();
            }
            let style = if row == selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            let marker = if row == selected { "  ▸ " } else { "    " };
            let display = self.loaded.config.subscriptions.display(meter);
            let reading = self.subscriptions.get(meter);
            lines.push(TLine::from(vec![
                Span::styled(
                    format!(
                        "{marker}{:<16} {:<8}",
                        meter.label(),
                        format!("{display:?}").to_ascii_lowercase()
                    ),
                    style,
                ),
                Span::styled(state(display, reading, now), dim),
            ]));
        }

        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("Sampling", heading)));
        lines.push(TLine::from(Span::styled(
            "  How often the switched-on meters are re-read.",
            dim,
        )));
        if selected == REFRESH_ROW {
            selected_line = lines.len();
        }
        let refresh_style = if selected == REFRESH_ROW {
            self.theme.selection()
        } else {
            Style::default()
        };
        let marker = if selected == REFRESH_ROW {
            "  ▸ "
        } else {
            "    "
        };
        lines.push(TLine::from(vec![
            Span::styled(
                format!(
                    "{marker}{:<16} {:<8}",
                    "Refresh",
                    refresh_label(self.loaded.config.subscriptions.refresh_seconds)
                ),
                refresh_style,
            ),
            Span::styled(self.sampled_note(now), dim),
        ]));

        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "j/k select · ←/→ or Enter change · r refresh now · applies live",
            dim,
        )));
        // Naming the variable rather than offering to store a key is the whole
        // policy, so it belongs on the page and not only in the config docs.
        lines.push(TLine::from(Span::styled(
            format!(
                "OpenRouter key read from ${} · Claude uses the CLI's own login",
                self.loaded
                    .config
                    .subscriptions
                    .open_router_key_env
                    .clone()
                    .unwrap_or_else(|| medulla::subscriptions::OPENROUTER_KEY_ENV.to_string())
            ),
            dim,
        )));
        let where_saved = match &self.config_path {
            Some(path) => format!("saved to {}", path.display()),
            None => "changes apply live (no config path set)".into(),
        };
        lines.push(TLine::from(Span::styled(where_saved, dim)));

        let height = inner.height as usize;
        let scroll = if selected_line < height {
            0
        } else {
            selected_line.saturating_sub(height / 2)
        };
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .scroll((scroll as u16, 0)),
            inner,
        );
    }

    /// What the sampling row reports beside the interval.
    fn sampled_note(&self, now: i64) -> String {
        if self.subscriptions_loading {
            return "sampling…".into();
        }
        match self.subscriptions.sampled_at {
            0 => "not sampled yet".into(),
            at => format!("last read {} ago", ago(now.saturating_sub(at))),
        }
    }
}

/// One meter's current state, as the settings page spells it.
///
/// A switched-off meter says nothing at all rather than "—": there is no
/// reading because nobody asked for one, and the two cases should not look the
/// same on the page where the asking is configured.
fn state(display: ResourceDisplay, reading: Option<&MeterReading>, now: i64) -> String {
    if display == ResourceDisplay::Off {
        return String::new();
    }
    let Some(reading) = reading else {
        return "waiting for first sample".into();
    };
    if let Some(note) = &reading.note {
        if !reading.has_value() {
            return note.clone();
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(fraction) = reading.used_fraction {
        parts.push(format!("{:.0}% used", fraction * 100.0));
    }
    if let Some(remaining) = reading.remaining {
        let money = |value| match reading.currency.as_deref().unwrap_or("USD") {
            "USD" => format!("${value:.2}"),
            "EUR" => format!("€{value:.2}"),
            "GBP" => format!("£{value:.2}"),
            code => format!("{code} {value:.2}"),
        };
        parts.push(match reading.limit {
            Some(limit) => format!("{} of {} left", money(remaining), money(limit)),
            None => format!("{} left", money(remaining)),
        });
    }
    if let Some(resets_at) = reading.resets_at {
        let seconds = resets_at.saturating_sub(now);
        parts.push(if seconds > 0 {
            format!("resets in {}", ago(seconds))
        } else {
            "reset due".into()
        });
    }
    if parts.is_empty() {
        return "no reading".into();
    }
    parts.join(" · ")
}

/// A duration in seconds as a compact `2d 3h` / `4h 5m` / `6m` string.
fn ago(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (days, hours, minutes) = (
        seconds / 86_400,
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
    );
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

/// The heading one source's meters sit under.
fn source_title(source: MeterSource) -> &'static str {
    match source {
        MeterSource::Claude => "Claude",
        MeterSource::Codex => "Codex",
        MeterSource::OpenRouter => "OpenRouter",
        MeterSource::Tinyhumans => "TinyHumans portal",
    }
}

/// Where that source's numbers come from, so an operator can tell a stale
/// reading from a broken one.
fn source_note(source: MeterSource) -> &'static str {
    match source {
        MeterSource::Claude => "Read from claude.ai with the local CLI's login.",
        MeterSource::Codex => "Read from the newest Codex transcript on this machine.",
        MeterSource::OpenRouter => "Prepaid balance, read with the key named below.",
        MeterSource::Tinyhumans => "Shares the balance the Usage page loads.",
    }
}
