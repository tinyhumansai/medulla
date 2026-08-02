//! Event routing and pointer input for [`App`]: the top-level [`App::on_event`]
//! dispatch, mouse scroll/click handling, tab hit-testing, and the small
//! navigation helpers (rail movement, prompt-history recall, mouse toggle).
//! Keyboard handling proper lives in [`super::keys`].

use crossterm::event::{Event, KeyEventKind, MouseButton, MouseEventKind};

use super::rail::RailRow;
use crate::ui::agents::{agent_row_model, AgentRole, AgentRow};
use crate::ui::composer::{insert_at, normalize_paste, Draft};

use super::types::{
    App, Cmd, MEMORY_SUBPAGES, ROUTING_SUBPAGES, SETTINGS_SUBPAGES, TASKS_SUBPAGES,
    TOKENMAXXING_SUBPAGES,
};

impl App {
    /// Route a terminal event to the key or mouse handler, producing any command
    /// the event loop must run.
    pub fn on_event(&mut self, ev: Event) -> Option<Cmd> {
        match ev {
            Event::Key(k) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.on_key(k)
            }
            Event::Mouse(m) => self.on_mouse(m),
            Event::Paste(text) => {
                self.on_paste(&text);
                None
            }
            _ => None,
        }
    }

    /// Insert a bracketed-paste payload into whatever text field has the caret.
    ///
    /// The point of routing paste as its own event is that the payload is never
    /// re-read as key presses: a newline inside it lands in the draft instead of
    /// reaching the `Enter` bindings, so a multi-line paste no longer submits
    /// itself — once per line, at that — and a `/`-prefixed paste no longer runs
    /// the command the peek happens to be highlighting. Only an explicit `Enter`
    /// submits.
    ///
    /// An open inline prompt takes the payload first, flattened to one line
    /// because that is all it can draw. Otherwise the Agents composer takes it,
    /// with `\r\n` and bare `\r` normalised to `\n`. Modals that own no text
    /// field (the resume picker, the decisions overlay) swallow the paste rather
    /// than let it land in a composer the operator cannot see.
    fn on_paste(&mut self, text: &str) {
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.paste(text);
            return;
        }
        if self.tab() != "Agents" || self.resume_picker.is_some() || self.decision_open {
            return;
        }
        self.draft = insert_at(&self.draft.text, self.draft.cursor, &normalize_paste(text));
        // Narrowing the command peek invalidates where its cursor pointed,
        // exactly as typing a character does.
        self.command_index = 0;
    }

    /// Handle scroll and left-click mouse events for the active tab.
    pub(super) fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Option<Cmd> {
        if self.resume_picker.is_some() {
            return None; // modal swallows mouse
        }
        match m.kind {
            MouseEventKind::ScrollUp => self.scroll_at(m.column, m.row, true),
            MouseEventKind::ScrollDown => self.scroll_at(m.column, m.row, false),
            // A press both acts on what is under it and arms a drag. The click
            // fires here rather than on release so navigation stays immediate;
            // a drag that follows selects text without undoing it.
            MouseEventKind::Down(MouseButton::Left) => {
                self.drag_anchor = Some((m.column, m.row));
                self.selection = None;
                return self.handle_click(m.column, m.row);
            }
            MouseEventKind::Drag(MouseButton::Left) => self.extend_selection(m.column, m.row),
            // Copy on release, the way tmux does it — no second keystroke to
            // confirm. The copy itself waits for the next draw, when the
            // selected cells can be read back out of the rendered buffer.
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag_anchor = None;
                self.copy_selection = self.selection.is_some();
            }
            _ => {}
        }
        None
    }

    /// Scroll whatever the pointer is over, rather than whatever has focus.
    ///
    /// A wheel event carries a position, so it should act on the list under it —
    /// scrolling the rail while the pointer sits on the transcript is the kind of
    /// thing that makes a mouse feel bolted on. Focus is left alone: the wheel
    /// looks around, it does not move where typing goes.
    pub(super) fn scroll_at(&mut self, x: u16, y: u16, up: bool) {
        // The subpage menu scrolls through its pages, whichever tab drew it.
        if self.hit_nav.area.contains((x, y).into()) {
            self.scroll_subpage(up);
            return;
        }
        match self.tab() {
            // Over the rail, the wheel walks the cursor over lanes and fleet
            // rows; anywhere else on the tab it scrolls the transcript.
            "Agents" => match self.hit_agents {
                Some((rail, _)) if rail.contains((x, y).into()) => self.move_agent_index(up),
                _ => self.scroll_transcript(up, 3),
            },
            "Memory" if self.memory_focused => {
                self.memory_index = if up {
                    self.memory_index.saturating_sub(1)
                } else {
                    (self.memory_index + 1).min(self.memory_page_entries().len().saturating_sub(1))
                };
            }
            // Trace and Context are Settings subpages, not tabs, so they are
            // matched on the subpage rather than on the tab — which is always
            // "Settings" for both.
            "Settings" => match self.settings_subpage() {
                "Trace" => {
                    self.selected = if up {
                        self.selected.saturating_sub(3)
                    } else {
                        self.selected + 3
                    }
                }
                "Context" => {
                    self.context_index = if up {
                        self.context_index.saturating_sub(1)
                    } else {
                        (self.context_index + 1).min(self.contexts.len().saturating_sub(1))
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Move the active tab's subpage selection by one, for a wheel over its menu.
    fn scroll_subpage(&mut self, up: bool) {
        let step = |index: usize, len: usize| {
            if up {
                index.saturating_sub(1)
            } else {
                (index + 1).min(len.saturating_sub(1))
            }
        };
        match self.tab() {
            "Tasks" => self.tasks_index = step(self.tasks_index, TASKS_SUBPAGES.len()),
            "TokenMaxxxing" => {
                self.tokenmaxxing_index =
                    step(self.tokenmaxxing_index, TOKENMAXXING_SUBPAGES.len());
            }
            "Routing" => self.routing_index = step(self.routing_index, ROUTING_SUBPAGES.len()),
            "Memory" => {
                self.memory_subpage_index = step(self.memory_subpage_index, MEMORY_SUBPAGES.len());
            }
            "Settings" => self.settings_index = step(self.settings_index, SETTINGS_SUBPAGES.len()),
            _ => {}
        }
    }

    /// Grow the selection block from the drag anchor to `(x, y)`.
    ///
    /// The block is normalized so a drag up or leftwards selects the same cells
    /// as the same drag made in reverse, and clamped to the pane the drag
    /// started in: panes sit side by side, so an unclamped block would splice
    /// the rail's labels into every line of the transcript beside it.
    pub(super) fn extend_selection(&mut self, x: u16, y: u16) {
        let Some((ax, ay)) = self.drag_anchor else {
            return;
        };
        let pane = self.pane_at(ax, ay);
        let clamp_x = |v: u16| v.clamp(pane.x, pane.right().saturating_sub(1));
        let clamp_y = |v: u16| v.clamp(pane.y, pane.bottom().saturating_sub(1));
        let (x, y) = (clamp_x(x), clamp_y(y));
        self.selection = Some((ax.min(x), ay.min(y), ax.max(x), ay.max(y)));
    }

    /// Resolve a left click at `(x, y)` to a tab switch or a row selection in the
    /// Agents, Context, or Chat panes.
    pub(super) fn handle_click(&mut self, x: u16, y: u16) -> Option<Cmd> {
        // Tab bar.
        if y == self.hit_tabs_row {
            for (i, (start, end)) in self.hit_tabs.clone().into_iter().enumerate() {
                if x >= start && x <= end {
                    self.tab_index = i;
                    self.selected = 0;
                    return self.tab_enter_cmd();
                }
            }
            return None;
        }
        let tab = self.tab();
        // Subpage nav. Clicking a page both selects it and takes focus, which is
        // what Enter does from the keyboard — a click that only moved the
        // highlight would leave the arrows driving the menu the pointer just
        // left.
        if let Some(page) = self.hit_nav.page_at(x, y) {
            match tab {
                "Tasks" => (self.tasks_index, self.tasks_focused) = (page, true),
                "TokenMaxxxing" => {
                    (self.tokenmaxxing_index, self.tokenmaxxing_focused) = (page, true);
                }
                "Routing" => (self.routing_index, self.routing_focused) = (page, true),
                "Memory" => (self.memory_subpage_index, self.memory_focused) = (page, true),
                "Settings" => (self.settings_index, self.settings_focused) = (page, true),
                _ => {}
            }
            return None;
        }
        if tab == "Agents" {
            // The rail stacks two hit boxes — threads above lanes — so both are
            // tried; an `else if` here would leave the strip unclickable.
            if let Some((rect, window_start)) = self.hit_threads {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    let idx = window_start + rel;
                    if let Some(t) = self.snapshot.threads.get(idx) {
                        let id = t.id.clone();
                        self.runtime.set_active_thread(id);
                        self.chat_scroll = 0;
                        self.agent_scroll = 0;
                        self.refresh_snapshot();
                    }
                }
            }
            if let Some((rect, window_start)) = self.hit_agents {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    // The *rail's* rows, not the lane rows: the rail also holds
                    // the dividers, the declared fleet, and the template
                    // catalog, and `agent_index` indexes all of it. Reading the
                    // shorter list here meant every click below the lanes fell
                    // off the end and silently did nothing.
                    let rows = self.rail_rows();
                    let idx = window_start + rel;
                    if let Some(row) = rows.get(idx) {
                        if row.selectable() {
                            self.agent_scroll = 0;
                            self.chat_scroll = 0;
                            self.agent_index = idx;
                            // A click is a focus gesture: the arrows should now
                            // continue from the row that was just picked.
                            self.focus_agents_rail();
                            // Clicking a task is the request to watch it.
                            if let Some(cmd) = self.retarget_watch() {
                                return Some(cmd);
                            }
                        }
                    }
                }
            }
        } else if tab == "Settings" && self.settings_subpage() == "Context" {
            // Context is a Settings subpage, not a tab — matching on `tab` here
            // made this branch unreachable, so clicking a chunk did nothing.
            if let Some(rect) = self.hit_context {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    if rel < self.contexts.len() {
                        self.context_index = rel;
                    }
                }
            }
        }
        None
    }

    /// The current Agents-list rows (lanes flattened with a hidden-row cap).
    pub(super) fn agent_rows(&self) -> Vec<AgentRow> {
        agent_row_model(&self.lanes(), 8)
    }

    /// The number of body rows a list pane can show for the current terminal
    /// height.
    pub(super) fn visible_count(&self) -> usize {
        (self.area.height as usize).saturating_sub(13).max(5)
    }

    /// Move the Agents-rail cursor to the next/previous selectable row.
    ///
    /// The rail spans the lanes and the declared fleet, so this walks straight
    /// from the last agent into the first host rather than stopping short.
    pub(super) fn move_agent_index(&mut self, up: bool) {
        let rows = self.rail_rows();
        if rows.is_empty() {
            return;
        }
        let clamped = self.agent_index.min(rows.len() - 1);
        let step: i64 = if up { -1 } else { 1 };
        let mut next = clamped as i64 + step;
        while next >= 0 && (next as usize) < rows.len() && !rows[next as usize].selectable() {
            next += step;
        }
        self.agent_index = if next < 0 || next as usize >= rows.len() {
            clamped
        } else {
            next as usize
        };
    }

    /// Open a new thread and focus the conversation.
    ///
    /// A thread is opened, not reset: several conversations can be in flight at
    /// once, and clearing the one you were in would throw away the transcript
    /// you were keeping. Nothing is inherited from the current thread — that is
    /// the whole difference from the fork this replaced.
    pub(super) fn new_thread(&mut self) {
        self.runtime.new_session();
        self.draft = crate::ui::composer::Draft::new();
        self.chat_scroll = 0;
        self.agent_scroll = 0;
        self.agent_index = 0;
        self.tab_index = super::types::tab_pos("Agents");
        self.refresh_snapshot();
        let name = self
            .snapshot
            .threads
            .get(self.active_thread_idx())
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "main".into());
        self.set_status(format!("Opened {name} · ^↑↓ switches threads"));
    }

    /// Recall an older prompt from history into the composer.
    pub(super) fn recall_older(&mut self) {
        let next = (self.history.len() as i64 - 1).min(self.history_index + 1);
        if next >= 0 {
            self.history_index = next;
            let recalled = self
                .history
                .get(self.history.len() - 1 - next as usize)
                .cloned()
                .unwrap_or_default();
            self.draft = Draft {
                cursor: recalled.chars().count(),
                text: recalled,
            };
        }
    }

    /// Recall a newer prompt from history (or clear back to an empty draft).
    pub(super) fn recall_newer(&mut self) {
        if self.history_index >= 0 {
            let next = self.history_index - 1;
            self.history_index = next;
            let recalled = if next >= 0 {
                self.history
                    .get(self.history.len() - 1 - next as usize)
                    .cloned()
                    .unwrap_or_default()
            } else {
                String::new()
            };
            self.draft = Draft {
                cursor: recalled.chars().count(),
                text: recalled,
            };
        }
    }

    /// Toggle mouse capture and note the new mode in the status line.
    pub(super) fn toggle_mouse(&mut self) {
        self.mouse_capture = !self.mouse_capture;
        self.set_status(if self.mouse_capture {
            "Mouse captured — click tabs/pages/lanes, wheel scrolls, drag to select and copy"
        } else {
            "Mouse released — native click-drag selection & copy restored"
        });
    }
}

impl App {
    /// Point the live screen subscription at whatever is selected now.
    ///
    /// Returns a command only when the target changed, so this is safe to call
    /// after every selection move: every subscribe carries `resync: true`, and
    /// re-issuing one per keystroke would make the worker resend a full frame
    /// each time — the expensive thing the protocol exists to avoid.
    ///
    /// Only a selected *task* subscribes. A worker lane names no task, and
    /// streams are addressed by task: there is nothing to ask for, and guessing
    /// one of a worker's tasks would watch work the operator did not point at.
    pub(super) fn retarget_watch(&mut self) -> Option<Cmd> {
        let desired = self.watch_target();
        if desired == self.watching {
            return None;
        }
        let stop = self.watching.take();
        self.watching = desired.clone();
        Some(Cmd::WatchTask {
            stop,
            start: desired,
        })
    }

    /// The `(worker address, task id)` the current selection asks to watch.
    fn watch_target(&self) -> Option<(String, String)> {
        // Only on the Agents tab: leaving it releases the subscription.
        if self.tab() != "Agents" {
            return None;
        }
        let rows = self.rail_rows();
        let row = rows.get(self.agent_index.min(rows.len().saturating_sub(1)))?;
        let RailRow::Agent(AgentRow::Sub {
            task, lane_index, ..
        }) = row
        else {
            return None;
        };
        let lanes = self.lanes();
        let lane = lanes.get(*lane_index)?;
        // `Agent` is main's name for a roster agent / delegated task / peer
        // session — the tiers above it (orchestrator, reasoning, compress) run
        // no watchable harness.
        if lane.role != AgentRole::Agent {
            return None;
        }
        let agent_id = lane.agent_id.as_deref()?;
        // Streams are addressed to the worker's tiny.place address; a lane
        // carries its roster id. A worker added by address is registered under
        // it, so that is the fallback.
        let address = self
            .runtime
            .workers()
            .into_iter()
            .find(|w| w.id == agent_id)
            .map(|w| w.address)
            .unwrap_or_else(|| agent_id.to_string());
        Some((address, task.task_id.clone()))
    }
}
