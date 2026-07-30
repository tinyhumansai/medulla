//! The Agents rail: the threads strip, and the list of lanes, fleet rows, and
//! agent templates beneath it.
//!
//! One cursor spans all of it (see [`rail`](crate::ui::app::rail)); this module
//! only draws what that cursor moves over, and formats each kind of row. The
//! per-lane glyph and status suffix live here too — they are the row's own
//! vocabulary, not the transcript's.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::agents::{AgentLane, AgentRole, AgentRow, TaskStatus};
use crate::ui::util::fmt_tokens;
use crate::worker::pty::{HarnessControl, SessionRow};

use super::super::super::rail::{RailRow, NEW_HARNESS_LABEL};
use super::super::super::types::App;
use super::super::color;
use super::types::{AgentsPanes, Selection};

/// The most content columns the Agents rail ever takes.
///
/// The rail is a list of short labels beside the surface the operator is
/// actually reading. Sizing it to its widest row alone let one long harness
/// path — an absolute working directory is easily eighty columns — push the
/// transcript into a gutter. Rows that do not fit within this wrap onto a second
/// line instead of buying width nothing else needs.
pub(super) const RAIL_MAX_CONTENT: usize = 36;

/// How far a wrapped row's continuation lines are indented, so a row that took
/// two lines still reads as one row rather than as two entries.
const CONT_INDENT: usize = 5;

impl App {
    /// Draw the threads strip and the rail list under it.
    pub(super) fn draw_agents_rail(
        &mut self,
        f: &mut Frame,
        panes: &AgentsPanes,
        selection: &Selection,
    ) {
        let threads = self.thread_rows();
        if threads.len() > 1 {
            self.draw_thread_strip(f, panes.threads, &threads);
        } else {
            self.hit_threads = None;
        }

        let running_tasks: usize = selection
            .lanes
            .iter()
            .map(|l| {
                l.tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Running)
                    .count()
            })
            .sum();
        let agents = selection
            .lanes
            .iter()
            .filter(|l| !l.role.is_function())
            .count();
        let title = if running_tasks > 0 {
            format!("Agents · {agents} · {running_tasks} running")
        } else {
            format!("Agents · {agents}")
        };
        // The border says which half the keyboard is driving. Without it, Esc
        // moving focus to the rail is invisible until the next arrow press.
        let block = crate::ui::widgets::panel(&self.theme, title, self.agents_rail_focused());
        let inner = block.inner(panes.rail);
        f.render_widget(block, panes.rail);

        // A row may occupy more than one line now, so the viewport is measured
        // in *lines* and each drawn line remembers the row it came from. Without
        // that map a click below a wrapped row would select its neighbour.
        let width = inner.width as usize;
        let mut lines: Vec<TLine> = Vec::new();
        let mut owners: Vec<usize> = Vec::new();
        let mut active_line = 0;
        for (index, row) in selection.rows.iter().enumerate() {
            if index == selection.active {
                active_line = lines.len();
            }
            for line in self.rail_row_lines(row, &selection.lanes, index == selection.active, width)
            {
                lines.push(line);
                owners.push(index);
            }
        }

        let capacity = (inner.height as usize).max(1);
        let start = crate::ui::selection::viewport_start(active_line, lines.len(), capacity);
        self.hit_agents = Some((
            inner,
            owners.iter().skip(start).take(capacity).copied().collect(),
        ));
        let view: Vec<TLine> = lines.into_iter().skip(start).take(capacity).collect();
        f.render_widget(Paragraph::new(Text::from(view)), inner);
    }

    /// One row per open thread, with its running/attention badges. Built apart
    /// from the draw so the rail can be sized to it.
    pub(super) fn thread_rows(&self) -> Vec<TLine<'static>> {
        self.snapshot
            .threads
            .iter()
            .map(|thread| {
                let marker = if thread.running { "▶" } else { "●" };
                let mut badges = Vec::new();
                if thread.running_tasks > 0 {
                    badges.push(format!("{} run", thread.running_tasks));
                }
                if thread.attention > 0 {
                    badges.push(format!("{}⚠", thread.attention));
                }
                let badge = if badges.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", badges.join(" "))
                };
                let mut style = Style::default();
                if thread.running {
                    style = style.fg(Color::Yellow);
                }
                if thread.id == self.snapshot.active_thread_id {
                    style = self.theme.selection();
                }
                TLine::from(Span::styled(
                    format!("{marker} {} · {}t{badge}", thread.name, thread.turns),
                    style,
                ))
            })
            .collect()
    }

    /// Draw the threads strip above the lane list.
    fn draw_thread_strip(&mut self, f: &mut Frame, area: Rect, threads: &[TLine<'static>]) {
        let block = self.panel(format!("Threads · {}", threads.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let capacity = (inner.height as usize).max(1);
        let active = self.active_thread_idx();
        let window_start = crate::ui::selection::viewport_start(active, threads.len(), capacity);
        self.hit_threads = Some((inner, window_start));
        let view: Vec<TLine> = threads
            .iter()
            .skip(window_start)
            .take(capacity)
            .cloned()
            .collect();
        f.render_widget(Paragraph::new(Text::from(view)), inner);
    }

    /// Format one rail row as the lines it occupies, wrapped to `width`.
    ///
    /// Most rows fit on one. A harness row is two or three by construction: its
    /// provider and who holds it on the first, its working directory under
    /// that — the directory is the only thing telling two harnesses of the same
    /// provider apart, so it has to be readable, and reading it used to cost
    /// the transcript however many columns the path happened to want. Anything
    /// else that overruns — a lane labelled with a full tiny.place address, a
    /// task row carrying a long work chip — wraps rather than being cut off at
    /// the border or widening the rail for everyone.
    pub(super) fn rail_row_lines(
        &self,
        row: &RailRow,
        lanes: &[AgentLane],
        active: bool,
        width: usize,
    ) -> Vec<TLine<'static>> {
        match row {
            RailRow::Harness(session) => self.own_harness_lines(session, active, width),
            other => wrap_line(
                &self.rail_row_line(other, lanes, active),
                width,
                CONT_INDENT,
            ),
        }
    }

    /// Format one single-line rail row.
    pub(super) fn rail_row_line(
        &self,
        row: &RailRow,
        lanes: &[AgentLane],
        active: bool,
    ) -> TLine<'static> {
        match row {
            RailRow::Agent(row) => self.agent_row_line(row, lanes, active),
            RailRow::NewHarness => self.new_harness_line(active),
            RailRow::HarnessSeparator => TLine::from(Span::styled(
                "── your harnesses ──",
                Style::default().add_modifier(Modifier::DIM),
            )),
            // Only reached through `rail_row_lines`, which draws a harness over
            // several lines; kept total so measurement can call either.
            RailRow::Harness(row) => self
                .own_harness_lines(row, active, RAIL_MAX_CONTENT)
                .into_iter()
                .next()
                .unwrap_or_default(),
        }
    }

    /// Format the `+ New harness` action row.
    ///
    /// Drawn as a button rather than as another list entry — bold and coloured,
    /// with its chord beside it — because it is the one row on the rail that
    /// *does* something rather than selecting something.
    fn new_harness_line(&self, active: bool) -> TLine<'static> {
        let style = if active {
            self.theme.selection()
        } else {
            Style::default()
                .fg(color("cyan"))
                .add_modifier(Modifier::BOLD)
        };
        TLine::from(vec![
            Span::styled(format!(" {NEW_HARNESS_LABEL} "), style),
            Span::styled(" ⏎ / ^T", Style::default().add_modifier(Modifier::DIM)),
        ])
    }

    /// Format one operator-started harness row over its lines.
    ///
    /// Says who holds it in words rather than only by colour: "unmanaged" is the
    /// whole reason the row exists, and an operator who hands one to the
    /// orchestrator needs to see that it took effect.
    fn own_harness_lines(
        &self,
        row: &SessionRow,
        active: bool,
        width: usize,
    ) -> Vec<TLine<'static>> {
        let control = match row.control {
            HarnessControl::User => " · unmanaged",
            HarnessControl::Orchestrator => " · orchestrator",
        };
        let style = if active {
            self.theme.selection()
        } else if row.control == HarnessControl::User {
            Style::default().fg(color("cyan"))
        } else {
            Style::default()
        };
        const INDENT: &str = "   ";
        const PATH_INDENT: &str = "     ";
        // Not clipped here: the terminal truncates an over-long line at the
        // pane edge anyway, and `clip` collapses whitespace — it would eat the
        // indent that sets a harness row apart from the divider above it.
        let head = format!(
            "{INDENT}{} {}{control}",
            row.state.glyph(),
            row.provider.as_str()
        );
        let mut lines = vec![TLine::from(Span::styled(head, style))];
        // The path is dimmed even under the cursor: it is the row's second
        // fact, and giving it the same weight as the provider makes a selected
        // harness read as two rows rather than one.
        let path_style = if active {
            style
        } else {
            style.add_modifier(Modifier::DIM)
        };
        let room = width.saturating_sub(PATH_INDENT.len()).max(6);
        for part in wrap_path(&short_home(&row.cwd), room, 2) {
            lines.push(TLine::from(Span::styled(
                format!("{PATH_INDENT}{part}"),
                path_style,
            )));
        }
        lines
    }

    /// Format one Agents-list row (separator, "more", sub-task, or lane).
    pub(super) fn agent_row_line(
        &self,
        row: &AgentRow,
        lanes: &[AgentLane],
        active: bool,
    ) -> TLine<'static> {
        match row {
            AgentRow::Separator => TLine::from(Span::styled(
                "── functions ──",
                Style::default().add_modifier(Modifier::DIM),
            )),
            AgentRow::More { hidden, .. } => TLine::from(Span::styled(
                format!("   └ +{hidden} more"),
                Style::default().add_modifier(Modifier::DIM),
            )),
            AgentRow::Sub { task, last, .. } => {
                let branch = if *last { "└" } else { "├" };
                let mut style = Style::default();
                if active {
                    style = self.theme.selection();
                }
                let status_style = if active {
                    style
                } else {
                    style.fg(color(task.status.color()))
                };
                // The chip is what makes a row worth reading at a glance: a
                // task that says "running · 12 turns" is indistinguishable from
                // every other one, while "3/7 ⑂2" says where it has got to.
                let chip = task
                    .work
                    .as_deref()
                    .map(crate::ui::work::work_chip)
                    .filter(|chip| !chip.is_empty())
                    .map(|chip| format!(" · {chip}"))
                    .unwrap_or_default();
                TLine::from(vec![
                    Span::styled(format!("   {branch} {} · ", task.task_id), style),
                    Span::styled(task.status.label().to_string(), status_style),
                    Span::styled(format!(" · {} turns{chip}", task.turns), style),
                ])
            }
            AgentRow::Lane { lane_index } => {
                let Some(item) = lanes.get(*lane_index) else {
                    return TLine::from("");
                };
                let window = self.loaded.config.medulla.context_window() as i64;
                let is_fn = item.role.is_function();
                let ctx = match item.context_tokens {
                    None => String::new(),
                    Some(used) if item.role == AgentRole::Agent => {
                        format!(" · ctx {}", fmt_tokens(used))
                    }
                    Some(used) => format!(
                        " · ctx {}/{} {}%",
                        fmt_tokens(used),
                        fmt_tokens(window),
                        ((used as f64 / window as f64) * 100.0).round() as i64
                    ),
                };
                let marker = self.lane_marker(item, is_fn);
                let state = self.lane_state(item);
                let sessions_note = if let Some(aid) = &item.agent_id {
                    let list = self.snapshot.sessions.get(aid).cloned().unwrap_or_default();
                    if list.is_empty() {
                        String::new()
                    } else {
                        let live = list.iter().filter(|s| s.state != "ended").count();
                        format!(" · {}/{} sess", live, list.len())
                    }
                } else {
                    String::new()
                };
                let mut style = Style::default().fg(color(item.role.color()));
                if is_fn {
                    style = style.add_modifier(Modifier::DIM);
                }
                if active {
                    style = self.theme.selection();
                }
                let work_note = item
                    .work
                    .as_deref()
                    .map(crate::ui::work::work_chip)
                    .filter(|chip| !chip.is_empty())
                    .map(|chip| format!(" · {chip}"))
                    .unwrap_or_default();
                crate::ui::agent_lane::line(
                    marker,
                    item.label.clone(),
                    format!(
                        " · {}{ctx}{state}{sessions_note}{work_note}",
                        item.turns.len()
                    ),
                    style,
                )
            }
        }
    }

    /// The presence/status glyph for a lane row.
    pub(in crate::ui::app::render) fn lane_marker(
        &self,
        item: &AgentLane,
        is_fn: bool,
    ) -> &'static str {
        if is_fn {
            "ƒ"
        } else if item.role != AgentRole::Agent {
            "●"
        } else if item.session_id.is_some() {
            let state = self.session_state(item);
            match state.as_deref() {
                Some("ended") => "○",
                _ => "●",
            }
        } else if let Some(aid) = &item.agent_id {
            match self.snapshot.presence.get(aid) {
                Some(p) => {
                    if p.online {
                        "●"
                    } else {
                        "○"
                    }
                }
                None if item.descriptor.is_some() => "◌",
                None => "◆",
            }
        } else if item.descriptor.is_some() {
            "◌"
        } else {
            "◆"
        }
    }

    /// The state of the session backing a lane, if any.
    pub(in crate::ui::app::render) fn session_state(&self, item: &AgentLane) -> Option<String> {
        let (sid, pid) = (item.session_id.as_ref()?, item.parent_agent_id.as_ref()?);
        self.snapshot
            .sessions
            .get(pid)?
            .iter()
            .find(|s| &s.id == sid)
            .map(|s| s.state.clone())
    }

    /// A short human-readable state suffix for a lane row.
    pub(in crate::ui::app::render) fn lane_state(&self, item: &AgentLane) -> String {
        if item.session_id.is_some() {
            let s = self.session_state(item);
            match s.as_deref() {
                Some("ended") => " · inactive".into(),
                Some(other) => format!(" · {other}"),
                None => " · …".into(),
            }
        } else if item.role == AgentRole::Agent {
            if item.active_tasks > 0 {
                " · busy".into()
            } else if item.turns.is_empty() {
                " · idle".into()
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    }
}

/// Re-flow one rail row across as many lines as `width` needs, keeping styles.
///
/// Works on characters rather than on the row's text, because a row is several
/// spans and its colours carry meaning — a task row's status word is coloured by
/// status, and re-wrapping through a plain `String` would hand the whole row one
/// style. Continuation lines are indented so a wrapped row still reads as one.
pub(super) fn wrap_line(line: &TLine<'static>, width: usize, indent: usize) -> Vec<TLine<'static>> {
    let width = width.max(8);
    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|span| span.content.chars().map(move |c| (c, span.style)))
        .collect();
    if cells.len() <= width {
        return vec![line.clone()];
    }
    let indent = indent.min(width.saturating_sub(4));
    let mut out = Vec::new();
    let mut start = 0;
    let mut first = true;
    while start < cells.len() {
        let pad = if first { 0 } else { indent };
        let room = width - pad;
        let end = (start + room).min(cells.len());
        // Break on the last space inside the window so a wrap falls between
        // words. A window with no space in it (a long address, a path) is cut
        // at the edge — there is nothing better to break on.
        let cut = if end == cells.len() {
            end
        } else {
            cells[start..end]
                .iter()
                .rposition(|(c, _)| *c == ' ')
                .map(|offset| start + offset)
                .filter(|&at| at > start)
                .unwrap_or(end)
        };
        out.push(styled_line(&cells[start..cut], pad));
        start = cut;
        // The space a break landed on is consumed by the break itself; leading
        // blanks on the next line would double the indent.
        while matches!(cells.get(start), Some((' ', _))) {
            start += 1;
        }
        first = false;
    }
    out
}

/// Rebuild a line from styled characters, merging neighbours that share a style.
pub(super) fn styled_line(cells: &[(char, Style)], pad: usize) -> TLine<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    for (c, style) in cells {
        match spans.last_mut() {
            // Neighbouring characters of one style are one span; the padding
            // span merges the same way when the first character is unstyled,
            // which is exactly what it should look like.
            Some(last) if last.style == *style => last.content.to_mut().push(*c),
            _ => spans.push(Span::styled(c.to_string(), *style)),
        }
    }
    TLine::from(spans)
}

/// A working directory with the operator's home collapsed to `~`.
///
/// Every harness on a laptop starts under the same home directory, so spelling
/// it out on every row spends columns on the one part of the path that
/// distinguishes nothing.
pub(super) fn short_home(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let trimmed = if trimmed.is_empty() { "/" } else { trimmed };
    let Some(home) = std::env::var_os("HOME") else {
        return trimmed.to_string();
    };
    let home = home.to_string_lossy().trim_end_matches('/').to_string();
    if home.is_empty() {
        return trimmed.to_string();
    }
    match trimmed.strip_prefix(&home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => trimmed.to_string(),
    }
}

/// Break a path across at most `max_lines` lines of `width` columns.
///
/// A path too long for that many lines loses leading segments, not trailing
/// ones: the tail names the checkout, while the head is the part every sibling
/// harness on the machine already shares. What was dropped is marked with a
/// leading `…` so the row never reads as an absolute path it is not.
pub(super) fn wrap_path(path: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(4);
    let max_lines = max_lines.max(1);
    let mut text = path.to_string();
    loop {
        let lines = flow_path(&text, width);
        if lines.len() <= max_lines {
            return lines;
        }
        // Drop the leading segment and try again. Each pass removes one, so
        // this reaches either a fit or a path with no separators left.
        let head = text.trim_start_matches('…');
        match head.split_once('/') {
            Some((_, tail)) if !tail.is_empty() => text = format!("…{tail}"),
            // A single unbreakable segment: it has no separator left to drop
            // a head at, so cut characters directly. The tail is what
            // distinguishes this checkout from a sibling sharing the same
            // prefix, so keep the last characters that fit and mark the
            // dropped head with a leading `…` rather than keeping the head
            // and losing the tail.
            _ => {
                let budget = width.saturating_mul(max_lines).saturating_sub(1).max(1);
                let chars: Vec<char> = head.chars().collect();
                let tail: String = if chars.len() > budget {
                    chars[chars.len() - budget..].iter().collect()
                } else {
                    head.to_string()
                };
                let mut lines = flow_path(&format!("…{tail}"), width);
                lines.truncate(max_lines);
                return lines;
            }
        }
    }
}

/// Lay a path out on `/` boundaries, one line per run of whole segments.
///
/// A path cut mid-segment is harder to read than one that wraps a segment
/// early, so a segment moves to the next line whole. One wider than the pane
/// has no separator to break on and is hard-cut rather than allowed to overflow.
pub(super) fn flow_path(path: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for segment in path.split_inclusive('/') {
        if !current.is_empty() && current.chars().count() + segment.chars().count() > width {
            out.push(std::mem::take(&mut current));
        }
        if segment.chars().count() > width {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let chars: Vec<char> = segment.chars().collect();
            for chunk in chars.chunks(width) {
                out.push(chunk.iter().collect());
            }
            continue;
        }
        current.push_str(segment);
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push("/".to_string());
    }
    out
}
