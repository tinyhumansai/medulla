//! The tree itself: one row per host, then one per agent under it.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use medulla::ui::hosts::{HostAgentRow, HostKind, HostRow};

use super::super::super::super::hosts::HostsRow;
use super::super::super::super::types::App;
use super::format::{dim, inline_text};

/// The action hints, split so a narrow terminal still shows the first line.
const FOOTER: &str =
    "↑↓/jk browse · → roles · n new agent · a add host · r refresh · Enter/s select · e edit · d/x remove";

impl App {
    /// Draw the host tree, windowed so the cursor stays visible.
    pub(super) fn draw_host_list(
        &mut self,
        f: &mut Frame,
        area: Rect,
        tree: &[HostRow],
        rows: &[HostsRow],
        selected: usize,
    ) {
        let agents: usize = tree.iter().map(|host| host.agents.len()).sum();
        let block = self.panel(format!("Hosts · {} · agents · {agents}", tree.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = Vec::new();
        if rows.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No hosts. This device is not hosting — open Add Host to connect a machine.",
                dim(),
            )));
        } else {
            // Reserve the footer (and optional hub identity) before choosing the
            // window, so the selected row and the action hints stay visible.
            let footer_rows = 1 + usize::from(self.snapshot.link.is_some()) * 2;
            let visible = usize::from(inner.height).saturating_sub(footer_rows).max(1);
            let start = crate::ui::selection::viewport_start(selected, rows.len(), visible);
            // Measured over the whole tree, not the visible window: columns that
            // resize as the list scrolls make the text appear to shift sideways
            // under a cursor that only moved down.
            let columns = Columns::measure(tree);
            for (index, row) in rows.iter().enumerate().skip(start).take(visible) {
                let Some(host) = tree.get(row.host) else {
                    continue;
                };
                let (text, mut style) = match row.agent.and_then(|at| host.agents.get(at)) {
                    Some(agent) => (
                        agent_line(
                            agent,
                            row.agent == Some(host.agents.len().saturating_sub(1)),
                            &columns,
                        ),
                        agent_style(agent),
                    ),
                    None => (host_line(host), host_style(host)),
                };
                if index == selected {
                    // While the roles toggle has focus the list still marks its
                    // row, but dimly — two lit cursors on one page read as two
                    // selections.
                    style = if self.host_roles_focus {
                        style.add_modifier(Modifier::BOLD)
                    } else {
                        self.theme.selection()
                    };
                }
                lines.push(TLine::from(Span::styled(text, style)));
            }
        }
        if let Some(identity) = &self.snapshot.link {
            lines.push(TLine::from(""));
            lines.push(TLine::from(vec![
                Span::styled("this hub · ", Style::default().fg(Color::Cyan)),
                Span::raw(identity.node_name.clone()),
            ]));
        }
        lines.push(TLine::from(Span::styled(FOOTER, dim())));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}

/// A host header: what it is called, where it is, and how many agents it holds.
///
/// A remote host says so on its own row rather than only in the preview: it is
/// the difference between a machine you can declare an agent on and one you can
/// only watch, and that must be legible without moving the cursor.
fn host_line(host: &HostRow) -> String {
    let kind = match host.kind {
        HostKind::Local => "local".to_string(),
        HostKind::Remote => "remote · read-only".to_string(),
    };
    let agents = match host.agents.len() {
        0 => "no agents".to_string(),
        1 => "1 agent".to_string(),
        count => format!("{count} agents"),
    };
    format!(
        "▾ {} · {} · {kind} · {agents}",
        inline_text(&host.label),
        inline_text(&host.id)
    )
}

/// What an agent row leads with: the name the operator gave it, else its id.
///
/// The name prompt at declaration time wrote to
/// [`AgentDeclaration::name`](medulla::runtime::AgentDeclaration), and the
/// projection carries it as [`HostAgentRow::label`] — but the row rendered the
/// id, so naming an agent looked like it had done nothing.
fn row_name(agent: &HostAgentRow) -> &str {
    let label = agent.label.trim();
    if label.is_empty() {
        agent.agent_id.trim()
    } else {
        label
    }
}

/// The width of each aligned column in the agent rows.
///
/// Measured across the *whole* tree rather than per host, which is the point:
/// columns that restart under every header are not columns, and comparing two
/// machines' agents means reading down a line rather than across a paragraph.
#[derive(Debug, Clone, Copy)]
struct Columns {
    /// Width of the agent-id column.
    id: usize,
    /// Width of the harness column.
    harness: usize,
    /// Width of the workspace column.
    workspace: usize,
}

/// Ceilings for the measured columns.
///
/// A single long id or a deep checkout path would otherwise set a width every
/// other row pays for, pushing the columns that carry the most meaning — roles,
/// and whether the agent is running at all — off a narrow terminal.
const MAX_ID: usize = 26;
const MAX_HARNESS: usize = 8;
const MAX_WORKSPACE: usize = 32;

impl Columns {
    /// Measure the columns the tree needs, each capped.
    fn measure(tree: &[HostRow]) -> Self {
        let agents = || tree.iter().flat_map(|host| host.agents.iter());
        Columns {
            id: agents()
                .map(|agent| inline_text(row_name(agent)).width())
                .max()
                .unwrap_or(0)
                .min(MAX_ID),
            harness: agents()
                .filter_map(|agent| agent.harness.as_deref())
                .map(|harness| inline_text(&harness.to_uppercase()).width())
                .max()
                .unwrap_or(0)
                .min(MAX_HARNESS),
            workspace: agents()
                .filter_map(|agent| agent.workspace.as_deref())
                .map(|workspace| inline_text(workspace).width())
                .max()
                .unwrap_or(0)
                .min(MAX_WORKSPACE),
        }
    }
}

/// Pad `value` out to `width`, truncating with `…` when it does not fit.
///
/// Measured in display columns, not bytes or `char`s: a CJK label is two
/// columns wide, and padding it by character count is what makes one row's
/// columns sit a cell to the left of every other row's.
fn cell(value: &str, width: usize) -> String {
    let have = value.width();
    if have <= width {
        return format!("{value}{}", " ".repeat(width - have));
    }
    let mut out = String::new();
    let mut used = 0;
    for c in value.chars() {
        let w = c.to_string().width();
        if used + w > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    format!("{out}{}", " ".repeat(width.saturating_sub(used + 1)))
}

/// Pad a path out to `width`, truncating from the *left* when it does not fit.
///
/// The tail of a checkout path is what identifies it — `…/medulla/src/tui`
/// says which tree it is, `/Users/sanil/Projects/…` says only that it is one
/// of many under the same parent.
fn path_cell(value: &str, width: usize) -> String {
    let have = value.width();
    if have <= width {
        return format!("{value}{}", " ".repeat(width - have));
    }
    let keep = width.saturating_sub(1);
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for c in value.chars().rev() {
        let w = c.to_string().width();
        if used + w > keep {
            break;
        }
        tail.push(c);
        used += w;
    }
    tail.reverse();
    let tail: String = tail.into_iter().collect();
    format!("…{tail}{}", " ".repeat(width.saturating_sub(used + 1)))
}

/// An agent under its host: the id a dispatch targets, its harness, where it
/// works, and the roles it is offered for.
///
/// Drawn as a branch of its host (`├─`, `└─` for the last one) with the fields
/// in fixed columns, so the shape of the fleet is readable down the page rather
/// than reconstructed from separators. Marked when it is the manual default
/// (`●`). A declared agent the roster has no entry for is flagged rather than
/// hidden: it is why nothing is being dispatched to it.
fn agent_line(agent: &HostAgentRow, last: bool, columns: &Columns) -> String {
    let branch = if last { "└─" } else { "├─" };
    let mark = if agent.selected { "●" } else { " " };
    let harness = agent
        .harness
        .as_deref()
        .map(|value| inline_text(&value.to_uppercase()))
        .unwrap_or_default();
    let workspace = agent
        .workspace
        .as_deref()
        .map(inline_text)
        .unwrap_or_default();
    let roles = match agent.roles.len() {
        0 => String::new(),
        1 => "1 role".to_string(),
        count => format!("{count} roles"),
    };
    let state = match (agent.live, agent.declared) {
        (false, _) => "declared, not running",
        (true, false) => "undeclared",
        (true, true) => "",
    };
    // The trailing columns are joined rather than padded — nothing lines up
    // after them, and padding the last cell only adds trailing blanks that
    // widen the selection highlight past the text.
    let tail: Vec<&str> = [roles.as_str(), state]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    let tail = match tail.is_empty() {
        true => String::new(),
        false => format!(" · {}", tail.join(" · ")),
    };
    // The id follows the name rather than replacing it: the name is what the
    // operator typed and recognises the row by, and the id is what a dispatch
    // targets — dropping either one loses a question the page is asked.
    let id = if row_name(agent) == agent.agent_id.trim() {
        String::new()
    } else {
        format!(" · {}", inline_text(&agent.agent_id))
    };
    format!(
        "  {branch} {mark} {} {} {}{id}{tail}",
        cell(&inline_text(row_name(agent)), columns.id),
        cell(&harness, columns.harness),
        path_cell(&workspace, columns.workspace),
    )
    .trim_end()
    .to_string()
}

/// A remote host reads dim: it is context, not something to act on.
fn host_style(host: &HostRow) -> Style {
    match host.kind {
        HostKind::Local => Style::default().fg(Color::Cyan),
        HostKind::Remote => dim().fg(Color::Cyan),
    }
}

/// The default agent is green; one that is declared but not running is dim,
/// because it is not a thing the orchestrator can reach right now.
fn agent_style(agent: &HostAgentRow) -> Style {
    if agent.selected {
        Style::default().fg(Color::Green)
    } else if !agent.live {
        dim()
    } else {
        Style::default()
    }
}
