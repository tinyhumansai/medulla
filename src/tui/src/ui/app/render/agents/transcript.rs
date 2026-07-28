//! The pane beside the rail: a transcript when the cursor is on a lane or task,
//! the declaration when it is on a fleet row.
//!
//! Above whichever it is sits the header — the harness task board, the seat
//! budget, where the lane runs, and the compact meters for that machine and
//! this lane's context window. The header is built first because it decides how
//! many rows are left for the body to scroll through.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::agents::{lane_lines, task_lines, Line as StyledLine};
use crate::ui::harness::{budget_note, task_board_lines};
use crate::ui::meters;
use medulla::harness_contract::AgentBudgetMetadata;

use super::super::super::types::App;
use super::super::{chat_lines, styled_to_tline};
use super::types::Selection;

impl App {
    /// Draw the transcript or declaration for whatever the cursor is on.
    pub(super) fn draw_agents_pane(&mut self, f: &mut Frame, area: Rect, selection: &Selection) {
        // A watched screen supersedes the transcript. The transcript says what a
        // worker reported; the screen shows what it is actually doing —
        // including the states it never reports at all, like a harness stopped
        // on a permission dialog, which writes no transcript records and so
        // reads as "thinking" until the task times out.
        if let Some(screen) = self.selected_screen(selection) {
            self.draw_worker_screen(f, area, &screen);
            return;
        }
        let lane = selection.lane();
        let pane_width = ((area.width as usize).saturating_sub(4)).max(24);
        let on_orchestrator = selection.on_orchestrator;
        let content_lines: Vec<StyledLine> = if let Some(t) = &selection.task {
            task_lines(t, pane_width)
        } else if on_orchestrator {
            // The orchestrator lane is the conversation: show what was said, not
            // the model calls that said it. The calls stay in Settings › Trace.
            chat_lines(&self.snapshot.events, pane_width)
        } else {
            lane_lines(lane, pane_width)
        };
        let title = if let Some(t) = &selection.task {
            format!(
                "{} › {} · {} turns",
                lane.map(|l| l.label.as_str()).unwrap_or("task"),
                t.task_id,
                t.turns
            )
        } else if on_orchestrator {
            let thread = self
                .snapshot
                .threads
                .get(self.active_thread_idx())
                .map(|t| t.name.clone())
                .unwrap_or_else(|| "main".into());
            format!(
                "orchestrator · {thread} · {} turns",
                self.snapshot.messages.len().div_ceil(2)
            )
        } else if let Some(l) = lane {
            format!("{} · {} turns", l.label, l.turns.len())
        } else {
            "Transcript".into()
        };
        let block = self.panel(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut header: Vec<TLine> = Vec::new();
        // What the selected agent is working on, in one line. The Work panel
        // beside this shows the whole picture, but it needs columns a narrow
        // terminal does not have — and the single most useful fact, what the
        // agent is on right now, fits here either way.
        if let Some(headline) = self
            .selected_work(selection)
            .and_then(crate::ui::work::work_headline)
        {
            header.push(TLine::from(Span::styled(
                format!("work · {headline}"),
                Style::default().fg(Color::Magenta),
            )));
        }
        // Harness task board: session-wide, shown only when the backend surfaces a
        // `HarnessStatus`. Degrades to nothing (empty vec) when absent or empty.
        if let Some(status) = &self.snapshot.harness {
            for line in task_board_lines(status, pane_width) {
                header.push(styled_to_tline(&line));
            }
        }
        // Read-only seat budget for the selected lane, when its descriptor carries
        // a `metadata.budget` stamp. Seat CRUD stays a backend REST concern.
        if let Some(budget) = lane
            .and_then(|l| l.descriptor.as_ref())
            .and_then(|d| AgentBudgetMetadata::from_metadata(&d.metadata))
        {
            header.push(TLine::from(Span::styled(
                format!("seat {}", budget_note(&budget)),
                Style::default().fg(if budget.exhausted {
                    Color::Red
                } else {
                    Color::Magenta
                }),
            )));
        }
        // Where this lane runs, and how hard: the placement chip, then compact
        // meters for the machine's memory and load and for the lane's own
        // context window. Every reading is omitted rather than zeroed when it
        // was not reported — a bar at 0% claims a measurement nobody took.
        {
            let capacity = self.fleet_capacity();
            if let Some(descriptor) = lane.and_then(|l| l.descriptor.as_ref()) {
                let placement = capacity.placement(descriptor);
                // Labelled, not bare: "this device · claude · /Users/me/repo"
                // reads as three unexplained tokens, and the two an operator
                // actually needs — which machine, which folder — are exactly the
                // two that look like everything else. The workspace path is the
                // agent's probed cwd, so it is the real working directory
                // rather than a declared intention.
                // A lane may have no place in the declared chain at all — a
                // hub-registered worker is announced directly and never appears
                // in the capacity snapshot until it answers a capability probe.
                // Falling back to what the descriptor itself carries means the
                // machine and the working directory show from the first frame
                // rather than only after the orchestrator happens to probe.
                let meta = |key: &str| {
                    descriptor
                        .metadata
                        .get(key)
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                };
                let host = placement
                    .host
                    .map(|h| h.name.clone())
                    .or_else(|| Some(descriptor.name.clone()).filter(|n| !n.trim().is_empty()));
                let workspace = placement
                    .workspace
                    .map(|w| w.path.clone())
                    .or_else(|| meta("workspace"));
                let chip = [
                    host.map(|h| format!("host {h}")),
                    placement
                        .harness
                        .map(|h| h.kind.clone())
                        .or_else(|| meta("harness")),
                    workspace.map(|w| format!("dir {w}")),
                    placement
                        .template
                        .map(|t| format!("via {}", t.name.clone().unwrap_or_else(|| t.id.clone()))),
                ]
                .into_iter()
                .flatten()
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
                if !chip.is_empty() {
                    header.push(TLine::from(Span::styled(
                        chip,
                        Style::default().fg(Color::Cyan),
                    )));
                }
                if let Some(resources) = placement.host.and_then(|h| h.resources.as_ref()) {
                    for line in [
                        meters::cpu_meter(resources),
                        meters::memory_meter(resources),
                        meters::disk_line(resources),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        header.push(styled_to_tline(&line));
                    }
                }
            }
            if let Some(lane) = lane {
                let window = self.loaded.config.medulla.context_window() as i64;
                if let Some(line) = meters::context_meter(&lane.usage, window) {
                    header.push(styled_to_tline(&line));
                }
            }
        }
        // Reserve the last row for the status line (scroll notice or spinner),
        // or a below-fold notice would itself fall below the fold.
        let capacity = (inner.height as usize)
            .saturating_sub(header.len() + 1)
            .max(4);
        let max_scroll = content_lines.len().saturating_sub(capacity);
        // The two transcripts keep their own scroll positions, so switching
        // lanes and switching back does not lose your place in either.
        let eff = if on_orchestrator {
            let eff = self.chat_scroll.min(max_scroll);
            self.chat_scroll = eff;
            eff
        } else {
            self.agent_scroll.min(max_scroll)
        };
        let end = content_lines.len() - eff;
        let view = &content_lines[end.saturating_sub(capacity)..end];
        let mut out = header;
        if view.is_empty() {
            out.push(TLine::from(Span::styled(
                "No messages yet — type below to start.",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        out.extend(view.iter().map(styled_to_tline));
        if eff > 0 {
            out.push(TLine::from(Span::styled(
                format!("↑ {eff} more line(s) below · k to catch up"),
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else if on_orchestrator && self.snapshot.running {
            let calls = crate::ui::stream::running_calls(&self.snapshot.events);
            let msg = if calls > 0 {
                format!(
                    "thinking · {calls} model call{} in flight",
                    if calls == 1 { "" } else { "s" }
                )
            } else {
                "working…".into()
            };
            out.push(TLine::from(Span::styled(
                format!(
                    "{} {msg}",
                    crate::ui::util::SPINNER[self.frame % crate::ui::util::SPINNER.len()],
                ),
                Style::default().fg(Color::Yellow),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(out)), inner);
    }
}

impl App {
    /// The screen held for whatever the cursor is on, if it is being watched.
    ///
    /// Only a selected *task* resolves one: streams are addressed by task id,
    /// which is the requester's own key, so a lane on its own names nothing to
    /// look up and guessing one of a worker's tasks would show work the
    /// operator did not point at.
    pub(super) fn selected_screen(
        &self,
        selection: &Selection,
    ) -> Option<medulla::hub::WatchedScreen> {
        let task = selection.task.as_ref()?;
        self.runtime
            .worker_screens()
            .into_iter()
            .find(|held| held.task_id == task.task_id)
    }

    /// Draw a watched worker's screen into `area`.
    ///
    /// No cursor is drawn and no scrollback is offered, matching the worker's
    /// own pane: this is a window, not a keyboard, and the protocol never
    /// synchronised history in the first place.
    fn draw_worker_screen(
        &mut self,
        f: &mut Frame,
        area: Rect,
        screen: &medulla::hub::WatchedScreen,
    ) {
        let age = medulla::clock::now_millis().saturating_sub(screen.updated_at);
        let block = self.panel(crate::ui::screen::screen_title(
            &screen.task_id,
            screen.seq,
            age,
        ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        // Clipped, never rewrapped. The grid is a framebuffer: reflowing it to
        // a narrower pane would not be the screen the worker is showing.
        f.render_widget(
            Paragraph::new(Text::from(crate::ui::screen::grid_lines(&screen.grid))),
            inner,
        );
    }
}
