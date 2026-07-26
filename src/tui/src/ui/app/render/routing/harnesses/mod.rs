//! The Harnesses page: connected runtime accounts, subdivided by host.
//!
//! A credential belongs to an account consumed by a harness, while the process
//! using it belongs to a host. The page therefore renders the hierarchy as
//! `harness kind → provider/account id → host`, with account-level usage shown
//! once even when the same account is connected on several machines.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::runtime::CapacitySnapshot;

use super::super::super::types::{App, CredentialStatus};
use grouping::account_groups;
use usage::usage_summary;

mod grouping;
#[cfg(test)]
mod tests;
mod types;
mod usage;

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

/// Harness kinds whose local credential presence Medulla knows how to detect.
const HARNESS_CREDENTIALS: [HarnessCredential; 3] = [
    HarnessCredential {
        kind: "claude-code",
        label: "Claude Code",
        subscription: Some(("Claude subscription", "run `claude /login`")),
        keys: &[("Anthropic API key", "ANTHROPIC_API_KEY")],
    },
    HarnessCredential {
        kind: "codex",
        label: "Codex",
        subscription: Some(("Codex subscription", "run `codex login`")),
        keys: &[("OpenAI API key", "OPENAI_API_KEY")],
    },
    HarnessCredential {
        kind: "opencode",
        label: "opencode",
        subscription: None,
        keys: &[
            ("OpenRouter API key", "OPENROUTER_API_KEY"),
            ("Anthropic API key", "ANTHROPIC_API_KEY"),
            ("OpenAI API key", "OPENAI_API_KEY"),
        ],
    },
];

impl App {
    /// Draw harnesses grouped by provider account, then by connected host.
    pub(super) fn draw_harnesses(&self, f: &mut Frame, area: Rect) {
        let capacity = self.fleet_capacity();
        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut lines: Vec<TLine> = Vec::new();

        for entry in &HARNESS_CREDENTIALS {
            lines.extend(harness_lines(
                &capacity,
                entry.kind,
                entry.label,
                Some((entry, &self.credential_status)),
            ));
        }

        // The harness vocabulary is open. Unknown kinds retain the same
        // account/host structure even though local credential detection has no
        // provider-specific login hint for them.
        for kind in unlisted_kinds(&capacity) {
            lines.extend(harness_lines(&capacity, &kind, &kind, None));
        }

        lines.push(TLine::from(Span::styled(
            "Press r to refresh connected hosts, accounts, usage, and local credential presence.",
            dim,
        )));
        lines.push(TLine::from(Span::styled(
            "Account IDs are opaque labels. Secret values are never rendered.",
            dim,
        )));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(self.panel("Harnesses")),
            area,
        );
    }
}

/// Render one harness kind's account groups and local credential fallbacks.
fn harness_lines(
    capacity: &CapacitySnapshot,
    kind: &str,
    label: &str,
    credentials: Option<(&HarnessCredential, &CredentialStatus)>,
) -> Vec<TLine<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = vec![TLine::from(Span::styled(label.to_string(), bold))];

    for group in account_groups(capacity, kind) {
        let account_id = group.account_id.as_deref().unwrap_or("unidentified");
        lines.push(TLine::from(Span::styled(
            format!(
                "  {} {} · {account_id}",
                title_case(&group.provider),
                group.account_type.replace('_', " ")
            ),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        if let Some(usage) = usage_summary(&group.budgets) {
            lines.push(TLine::from(Span::styled(
                format!("    usage · {usage}"),
                dim,
            )));
        }
        for harness in group.harnesses {
            let host = capacity
                .host(&harness.host_id)
                .map(|host| host.name.clone())
                .unwrap_or_else(|| format!("{} (undeclared)", harness.host_id));
            let connection = if harness.availability.eq_ignore_ascii_case("offline") {
                "offline"
            } else {
                "connected"
            };
            let readiness = if harness.ready { "ready" } else { "not ready" };
            let style = if harness.ready {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            lines.push(TLine::from(Span::styled(
                format!("    ▸ {host} · {connection} · {readiness}"),
                style,
            )));
        }
    }

    if let Some((entry, status)) = credentials {
        lines.push(TLine::from(Span::styled(
            "  detected on this host (account ID unavailable)",
            dim,
        )));
        if let Some((credential_label, hint)) = entry.subscription {
            lines.push(credential_line(
                credential_label,
                status.subscription_for(entry.kind),
                hint,
            ));
        }
        for (credential_label, variable) in entry.keys {
            lines.push(credential_line(
                credential_label,
                status.key_for(variable),
                &format!("set ${variable}"),
            ));
        }
    } else if capacity
        .harnesses
        .iter()
        .any(|harness| harness.kind == kind)
    {
        lines.push(TLine::from(Span::styled(
            "  local credential detection is unavailable for this harness",
            dim,
        )));
    }
    lines.push(TLine::from(""));
    lines
}

/// Declared harness kinds with no credential mapping, in first-seen order.
fn unlisted_kinds(capacity: &CapacitySnapshot) -> Vec<String> {
    let mut kinds = Vec::new();
    for harness in &capacity.harnesses {
        let known = HARNESS_CREDENTIALS
            .iter()
            .any(|entry| entry.kind == harness.kind);
        if !known && !kinds.contains(&harness.kind) {
            kinds.push(harness.kind.clone());
        }
    }
    kinds
}

/// Uppercase the first character of an open-vocabulary provider label.
fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Unknown".into(),
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
    fn key_for(&self, variable: &str) -> bool {
        match variable {
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
        Span::styled(format!("    {glyph} {label} · "), style),
        Span::styled(status, style),
    ])
}
