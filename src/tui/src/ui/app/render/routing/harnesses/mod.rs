//! The Harnesses page: the agent CLI runtimes work actually executes on, and
//! the credentials and budgets each one spends.
//!
//! Keys and subscriptions live here rather than on a page of their own because
//! that is where they belong in the model: a Claude subscription or an
//! `ANTHROPIC_API_KEY` is a property of the runtime that spends it, not of the
//! machine it happens to sit on. One host can run three harnesses on three
//! different providers, and one subscription can back harnesses on several
//! hosts, so a flat "credentials" list could only ever describe one of those.
//!
//! The page therefore reads per harness *kind*: what authenticates it, whether
//! this machine has that, and which declared instances of the kind exist — with
//! the host they run on and the token budgets they report.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::runtime::CapacitySnapshot;

use super::super::super::types::{App, CredentialStatus};

#[cfg(test)]
mod tests;

/// One harness kind's credential story: what authenticates it, and how.
struct HarnessCredential {
    /// The `HarnessDescriptor.kind` this describes.
    kind: &'static str,
    /// Operator-facing runtime name.
    label: &'static str,
    /// The subscription that backs it, when the CLI owns a login session.
    subscription: Option<(&'static str, &'static str)>,
    /// API keys that can back it instead, as `(label, env var)`.
    keys: &'static [(&'static str, &'static str)],
}

/// The harness kinds this client knows how to authenticate.
///
/// The harness vocabulary is open, not an enum, so a declared harness of an
/// unlisted kind is normal rather than an error: it gets no credential rows and
/// its declarations still render, under its own heading.
const HARNESS_CREDENTIALS: [HarnessCredential; 3] = [
    HarnessCredential {
        kind: "claude-code",
        label: "Claude Code",
        subscription: Some(("Claude subscription", "run `claude /login`")),
        keys: &[("Anthropic", "ANTHROPIC_API_KEY")],
    },
    HarnessCredential {
        kind: "codex",
        label: "Codex",
        subscription: Some(("Codex subscription", "run `codex login`")),
        keys: &[("OpenAI", "OPENAI_API_KEY")],
    },
    HarnessCredential {
        kind: "opencode",
        label: "opencode",
        subscription: None,
        keys: &[
            ("OpenRouter", "OPENROUTER_API_KEY"),
            ("Anthropic", "ANTHROPIC_API_KEY"),
            ("OpenAI", "OPENAI_API_KEY"),
        ],
    },
];

impl App {
    /// Draw each harness kind with its credentials, instances, and budgets.
    pub(super) fn draw_harnesses(&self, f: &mut Frame, area: Rect) {
        let capacity = self.fleet_capacity();
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let mut lines: Vec<TLine> = Vec::new();

        lines.push(TLine::from(Span::styled(
            "Custom OpenRouter harnesses",
            bold,
        )));
        if self.custom_harnesses.is_empty() {
            lines.push(TLine::from(Span::styled(
                "  No custom harnesses · press a to add one",
                dim,
            )));
        } else {
            let selected = self
                .custom_harness_index
                .min(self.custom_harnesses.len().saturating_sub(1));
            for (index, harness) in self.custom_harnesses.iter().enumerate() {
                let key_present = self.credential_status.key_for(&harness.api_key_env);
                let availability = if key_present {
                    "key connected"
                } else {
                    "key missing"
                };
                let marker = if index == selected { "▸" } else { " " };
                let row = format!(
                    "{marker} {} · {} · {} · host {} · {}",
                    harness.name,
                    harness.base_harness.display_name(),
                    harness.model,
                    harness.host_id,
                    availability,
                );
                let style = if index == selected {
                    self.theme.selection()
                } else if key_present {
                    Style::default().fg(Color::Green)
                } else {
                    dim
                };
                lines.push(TLine::from(Span::styled(row, style)));
            }
        }
        lines.push(TLine::from(Span::styled(
            "  ↑↓/jk select · a add · e edit · d delete · r refresh",
            dim,
        )));
        lines.push(TLine::from(Span::styled(
            "  Changes apply after the local host restarts.",
            dim,
        )));
        lines.push(TLine::from(""));

        for entry in &HARNESS_CREDENTIALS {
            lines.push(TLine::from(Span::styled(entry.label, bold)));
            if let Some((label, hint)) = entry.subscription {
                lines.push(credential_line(
                    label,
                    self.credential_status.subscription_for(entry.kind),
                    hint,
                ));
            }
            for (label, var) in entry.keys {
                lines.push(credential_line(
                    label,
                    self.credential_status.key_for(var),
                    &format!("set ${var}"),
                ));
            }
            lines.extend(instance_lines(&capacity, entry.kind));
            lines.push(TLine::from(""));
        }

        // Declared harnesses of a kind this client has no credential story for.
        // They are still real capacity, so they are listed rather than dropped.
        for kind in unlisted_kinds(&capacity) {
            lines.push(TLine::from(Span::styled(kind.clone(), bold)));
            lines.push(TLine::from(Span::styled(
                "  no credential mapping known — authentication is the runtime's own",
                dim,
            )));
            lines.extend(instance_lines(&capacity, &kind));
            lines.push(TLine::from(""));
        }

        lines.push(TLine::from(Span::styled(
            "Press r to refresh detected credentials.",
            dim,
        )));
        lines.push(TLine::from(Span::styled(
            "Secret values are never rendered. Subscription sessions remain owned by their provider CLIs.",
            dim,
        )));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(self.panel("Harness Types")),
            area,
        );
    }
}

/// The declared instances of one harness kind, with host, readiness, and budget.
///
/// Empty when nothing of this kind is declared: a credential can sit on a
/// machine that runs no harness, and printing "none declared" under every kind
/// an operator never set up would drown the page.
fn instance_lines(capacity: &CapacitySnapshot, kind: &str) -> Vec<TLine<'static>> {
    capacity
        .harnesses
        .iter()
        .filter(|harness| harness.kind == kind)
        .map(|harness| {
            let mut parts = vec![capacity
                .host(&harness.host_id)
                .map(|h| h.name.clone())
                .unwrap_or_else(|| format!("{} (undeclared)", harness.host_id))];
            parts.push(if harness.ready { "ready" } else { "not ready" }.to_string());
            if let Some(budget) = harness.budgets.iter().find(|b| b.remaining().is_some()) {
                parts.push(format!(
                    "{} {} · {} left",
                    budget.provider,
                    budget.window,
                    budget.remaining().map(tokens).unwrap_or_default()
                ));
            }
            TLine::from(Span::styled(
                format!("  ▸ {}", parts.join(" · ")),
                Style::default().fg(if harness.ready {
                    Color::Blue
                } else {
                    Color::Red
                }),
            ))
        })
        .collect()
}

/// Declared harness kinds with no credential mapping, in first-seen order.
fn unlisted_kinds(capacity: &CapacitySnapshot) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for harness in &capacity.harnesses {
        let known = HARNESS_CREDENTIALS.iter().any(|e| e.kind == harness.kind);
        if !known && !out.contains(&harness.kind) {
            out.push(harness.kind.clone());
        }
    }
    out
}

/// A token magnitude, matching the Fleet page's wording.
fn tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{}k", (n as f64 / 1_000.0).round() as u64)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

impl CredentialStatus {
    /// Whether the CLI-owned login session backing `kind` is present.
    fn subscription_for(&self, kind: &str) -> bool {
        match kind {
            "claude-code" => self.claude_subscription,
            "codex" => self.codex_subscription,
            _ => false,
        }
    }

    /// Whether the named API-key environment variable is present.
    fn key_for(&self, var: &str) -> bool {
        match var {
            "ANTHROPIC_API_KEY" => self.anthropic_api_key,
            "OPENAI_API_KEY" => self.openai_api_key,
            "OPENROUTER_API_KEY" => self.openrouter_api_key,
            _ => false,
        }
    }
}

/// Render one credential status without revealing its value.
fn credential_line<'a>(label: &str, present: bool, absent_hint: &str) -> TLine<'a> {
    let (glyph, status, style) = if present {
        (
            "●",
            "connected".to_string(),
            Style::default().fg(Color::Green),
        )
    } else {
        (
            "○",
            format!("not connected · {absent_hint}"),
            Style::default().add_modifier(Modifier::DIM),
        )
    };
    TLine::from(vec![
        Span::styled(format!("  {glyph} {label} · "), style),
        Span::styled(status, style),
    ])
}
