//! The Agents rail: the threads strip, and the list of lanes, fleet rows, and
//! agent templates beneath it.
//!
//! One cursor spans all of it (see [`rail`](crate::ui::app::rail)); this module
//! only draws what that cursor moves over, and formats each kind of row. The
//! per-lane glyph and status suffix live here too — they are the row's own
//! vocabulary, not the transcript's. Line-wrapping and path-shortening are
//! their own concern, kept in [`wrap`] so this file stays about *what* a row
//! says rather than how it is laid out across columns.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::ui::agents::{AgentLane, AgentRole, AgentRow, TaskStatus};
use crate::ui::util::{clip, fmt_tokens};
use crate::worker::pty::{HarnessAttention, HarnessControl, SessionRow, ATTENTION_GLYPH};

use super::super::super::rail::{RailRow, NEW_HARNESS_LABEL};
use super::super::super::types::App;
use super::super::color;
use super::types::{AgentsPanes, Selection};

mod status;
#[cfg(test)]
mod tests;
mod wrap;

use status::HarnessVisualState;
use wrap::{home_dir, short_home, wrap_line, wrap_path};

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
        // Every waiting harness on the device, including the ones inside lanes
        // rather than on rows of their own — the same number the tab badge
        // carries, so the two can never disagree.
        let waiting = self.harnesses_waiting();
        let mut title = if running_tasks > 0 {
            format!("Agents · {agents} · {running_tasks} running")
        } else {
            format!("Agents · {agents}")
        };
        // Last, and in the same vocabulary as the rows: a scrolled-away harness
        // stuck on a prompt is otherwise invisible until you find it.
        if waiting > 0 {
            title.push_str(&format!(" · {ATTENTION_GLYPH} {waiting} waiting on you"));
        }
        // The border says which half the keyboard is driving. Without it, Esc
        // moving focus to the rail is invisible until the next arrow press.
        let block = crate::ui::widgets::panel(&self.theme, title, self.agents_rail_focused());
        let inner = block.inner(panes.rail);
        f.render_widget(block, panes.rail);

        // A row may occupy more than one line now, so the viewport is measured
        // in *lines* and each drawn line remembers the row it came from. Without
        // that map a click below a wrapped row would select its neighbour.
        let width = inner.width as usize;
        // Collect waiting session IDs once so render code can check membership
        // instead of acquiring a lock per lane.
        let waiting_sessions = self.harnesses
            .as_ref()
            .map(|h| h.sessions.waiting_sessions())
            .unwrap_or_default();
        let mut lines: Vec<TLine> = Vec::new();
        let mut owners: Vec<usize> = Vec::new();
        let mut active_line = 0;
        for (index, row) in selection.rows.iter().enumerate() {
            if index == selection.active {
                active_line = lines.len();
            }
            for line in self.rail_row_lines(row, &selection.lanes, index == selection.active, width, &waiting_sessions)
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
        waiting_sessions: &std::collections::HashSet<String>,
    ) -> Vec<TLine<'static>> {
        match row {
            RailRow::Harness(session) => self.own_harness_lines(session, active, width),
            other => wrap_line(
                &self.rail_row_line(other, lanes, active, waiting_sessions),
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
        waiting_sessions: &std::collections::HashSet<String>,
    ) -> TLine<'static> {
        match row {
            RailRow::Agent(row) => self.agent_row_line(row, lanes, active, waiting_sessions),
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

    /// Format one operator-started harness as one compact rail row.
    ///
    /// Says who holds it in words rather than only by colour: "unmanaged" is the
    /// whole reason the row exists, and an operator who hands one to the
    /// orchestrator needs to see that it took effect. The working directory is
    /// shortened to the remaining width rather than consuming another row.
    fn own_harness_lines(
        &self,
        row: &SessionRow,
        active: bool,
        width: usize,
    ) -> Vec<TLine<'static>> {
        // A harness waiting on the operator is the one thing on this rail worth
        // interrupting them for, so it takes the row over: its own glyph, its
        // own colour, and a blink. Everything else the row says — who holds it,
        // which branch, which directory — is still true and still there, just no
        // longer what the eye lands on first.
        let waiting = self.harness_attention(row);
        let control = match row.control {
            HarnessControl::User => " · unmanaged",
            HarnessControl::Orchestrator => " · orchestrator",
        };
        let style = if active {
            self.theme.selection()
        } else if waiting.is_some() {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK)
        } else if row.control == HarnessControl::User {
            Style::default().fg(color("cyan"))
        } else {
            Style::default()
        };
        let glyph = match &waiting {
            // Not the state glyph: the session *is* running, and saying so here
            // would be answering a question nobody is asking about a row that
            // needs something done to it.
            Some(_) => ATTENTION_GLYPH,
            None => row.state.glyph(),
        };
        let head = format!("{glyph} {}{control}", row.provider.as_str());
        let head = if width == 0 {
            String::new()
        } else {
            clip(&head, width)
        };
        let detail_style = if active {
            style
        } else {
            style.add_modifier(Modifier::DIM)
        };
        const SEPARATOR: &str = " · ";
        let mut spans = vec![Span::styled(head, style)];
        let mut used = spans[0].width();
        let appearance = &self.loaded.config.appearance;
        if appearance.show_harness_branch {
            if let Some(branch) = row.branch.as_deref() {
                let remaining = width.saturating_sub(used + SEPARATOR.width());
                let reserve_for_path = if appearance.show_harness_path { 7 } else { 0 };
                let branch_room = remaining.saturating_sub(reserve_for_path).min(16);
                if branch_room >= 2 {
                    let branch = clip(branch, branch_room);
                    used += SEPARATOR.width() + branch.width();
                    spans.push(Span::styled(format!("{SEPARATOR}{branch}"), detail_style));
                }
            }
        }
        let path_room = width.saturating_sub(used + SEPARATOR.width());
        if appearance.show_harness_path && path_room >= 4 {
            let path = short_home(&row.cwd, home_dir().as_deref());
            let path = wrap_path(&path, path_room, 1).concat();
            spans.push(Span::styled(format!("{SEPARATOR}{path}"), detail_style));
        }
        let mut lines = vec![TLine::from(spans)];
        // On its own line, in words. The colour and the blink say *that* it
        // wants you; only the sentence says what for — and "approve a command"
        // and "it rang the bell" are worth different amounts of hurry.
        if let Some(cue) = waiting {
            let text = format!(
                "  {ATTENTION_GLYPH} {}",
                cue.label(medulla::clock::now_millis())
            );
            lines.extend(wrap_line(
                &TLine::from(Span::styled(clip(&text, width.max(1)), style)),
                width,
                CONT_INDENT,
            ));
        }
        lines
    }

    /// The cue a harness row should blink about, if it should.
    ///
    /// The attached session never blinks: its screen is the thing the operator
    /// is looking at, and a row shouting about a prompt that is already filling
    /// the pane beside it is noise. Nor does an exited one — whatever it was
    /// asking, nobody can answer it now.
    fn harness_attention(&self, row: &SessionRow) -> Option<HarnessAttention> {
        if !row.state.is_running() || self.harness_focus.is_attached_to(&row.id) {
            return None;
        }
        row.attention.clone()
    }

    /// Format one Agents-list row (separator, "more", sub-task, or lane).
    pub(super) fn agent_row_line(
        &self,
        row: &AgentRow,
        lanes: &[AgentLane],
        active: bool,
        waiting_sessions: &std::collections::HashSet<String>,
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
                let state = self.lane_state(item, waiting_sessions);
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
                let style = self.lane_style(item, is_fn, active);
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
    pub(in crate::ui::app::render) fn lane_state(&self, item: &AgentLane, waiting_sessions: &std::collections::HashSet<String>) -> String {
        if item.session_id.is_some() {
            self.session_state(item)
                .map(|_| format!(" · {}", self.harness_visual_state(item, waiting_sessions).label()))
                .unwrap_or_else(|| " · …".into())
        } else if item.role == AgentRole::Agent {
            format!(" · {}", self.harness_visual_state(item, waiting_sessions).label())
        } else {
            String::new()
        }
    }

    /// Style a lane while preserving both selection visibility and harness state.
    fn lane_style(&self, item: &AgentLane, is_fn: bool, active: bool) -> Style {
        let mut style = if active {
            self.theme.selection()
        } else {
            Style::default()
        };
        if item.role == AgentRole::Agent {
            let state = self.harness_visual_state(item);
            style = style.fg(state.color());
            if state.flashes() {
                style = style.add_modifier(Modifier::SLOW_BLINK);
            }
        } else {
            style = style.fg(color(item.role.color()));
        }
        if is_fn {
            style = style.add_modifier(Modifier::DIM);
        }
        style
    }

    /// Resolve the current state instead of letting an older terminal task
    /// override newer work. Pending input wins over active work, then the most
    /// recent terminal task supplies completion or error.
    ///
    /// A local harness stopped on its own permission prompt outranks all of it.
    /// The event fold cannot see that state — the harness writes no record while
    /// it waits — so a lane whose pty is sitting on "Do you want to proceed?"
    /// reads as *running* to everything else here, right up until the task times
    /// out. The screen knows, and this is where it gets to say so.
    fn harness_visual_state(&self, item: &AgentLane, waiting_sessions: &std::collections::HashSet<String>) -> HarnessVisualState {
        if self.lane_has_attention(item, waiting_sessions) {
            return HarnessVisualState::NeedsInput;
        }
        let session = self.session_state(item);
        status::classify(item, session.as_deref())
    }

    /// Whether a lane has a waiting harness by checking the pre-resolved waiting
    /// set, avoiding per-lane session locking.
    fn lane_has_attention(&self, item: &AgentLane, waiting_sessions: &std::collections::HashSet<String>) -> bool {
        let harnesses = match self.harnesses.as_ref() {
            Some(h) => h,
            None => return false,
        };
        item.tasks
            .iter()
            .filter_map(|task| harnesses.session_for_task(&task.task_id))
            .filter(|session| !self.harness_focus.is_attached_to(session))
            .any(|session| waiting_sessions.contains(&session))
    }
}
