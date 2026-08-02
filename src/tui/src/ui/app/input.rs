//! Event routing and pointer input for [`App`]: the top-level [`App::on_event`]
//! dispatch, mouse scroll/click handling, tab hit-testing, and the small
//! navigation helpers (rail movement, prompt-history recall, mouse toggle).
//! Keyboard handling proper lives in [`super::keys`].

use crossterm::event::{Event, KeyEventKind, MouseButton, MouseEventKind};

use super::rail::RailRow;
use crate::ui::agents::{agent_row_model, AgentRole, AgentRow};
use crate::ui::composer::{insert_at, normalize_paste, Draft};

use super::types::{App, Cmd, ROUTING_SUBPAGES, SETTINGS_SUBPAGES, TOKENMAXXING_SUBPAGES};

/// Rows one wheel notch moves, matching the transcript's own step so the two
/// panes feel like one surface. Only used for a harness we scroll ourselves —
/// one that takes mouse reports decides what a notch means itself.
const SCROLL_ROWS: usize = 3;

/// Translate a crossterm pointer event into the button and motion a mouse
/// report names, or `None` for one that is not a button event.
///
/// Bare motion with no button held (`Moved`) is excluded on purpose. Only
/// `DECSET 1003` asks for it, almost nothing negotiates that, and forwarding it
/// would put a report on the wire for every cell the pointer crosses the pane.
fn pointer_report(
    kind: MouseEventKind,
) -> Option<(
    crate::ui::harness_pane::mouse::Button,
    crate::ui::harness_pane::mouse::Motion,
)> {
    use crate::ui::harness_pane::mouse::{Button, Motion};
    let button = |b: MouseButton| match b {
        MouseButton::Left => Button::Left,
        MouseButton::Middle => Button::Middle,
        MouseButton::Right => Button::Right,
    };
    match kind {
        MouseEventKind::Down(b) => Some((button(b), Motion::Press)),
        MouseEventKind::Up(b) => Some((button(b), Motion::Release)),
        MouseEventKind::Drag(b) => Some((button(b), Motion::Drag)),
        _ => None,
    }
}

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
    /// Who takes it follows [`App::on_key`]'s precedence exactly, because a
    /// paste that landed somewhere the keyboard is not would be a payload the
    /// operator never sees again. So: the hand-back question first, since it is
    /// asked while still attached and the chrome holds the keyboard until it is
    /// answered; then the attached harness, which owns the keyboard outright and
    /// therefore owns the paste; then an open inline prompt, flattened to one
    /// line because that is all it can draw. Otherwise the Agents composer takes
    /// it, with `\r\n` and bare `\r` normalised to `\n`. Modals that own no text
    /// field (the harness and resume pickers, the decisions overlay) swallow the
    /// paste for the same reason they swallow the keyboard.
    fn on_paste(&mut self, text: &str) {
        if self.handback_prompt.is_some() {
            return;
        }
        if let Some(session) = self.harness_focus.attached_to().map(str::to_string) {
            self.paste_into_harness(&session, text);
            return;
        }
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.paste(text);
            return;
        }
        if self.tab() != "Agents" || self.overlay_owns_keys() {
            return;
        }
        self.draft = insert_at(&self.draft.text, self.draft.cursor, &normalize_paste(text));
        // Narrowing the command peek invalidates where its cursor pointed,
        // exactly as typing a character does.
        self.command_index = 0;
    }

    /// Handle scroll and left-click mouse events for the active tab.
    pub(super) fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Option<Cmd> {
        // A modal swallows the mouse, the same way it swallows the keyboard.
        // The harness picker is one: a click that navigated the rail behind it
        // left an overlay on screen describing a row nobody was pointing at.
        if self.resume_picker.is_some() || self.harness_picker.is_some() {
            return None;
        }
        // An attached harness is a terminal, and a terminal owns the pointer
        // over it exactly as it owns the keyboard. Placed ahead of everything
        // else for the same reason `handle_harness_key` runs first: attached is
        // a mode, and a mode that keeps a few gestures for itself is not one.
        if self.forward_harness_mouse(&m) {
            return None;
        }
        match m.kind {
            MouseEventKind::ScrollUp => self.scroll_at(m.column, m.row, true),
            MouseEventKind::ScrollDown => self.scroll_at(m.column, m.row, false),
            // A press both acts on what is under it and arms a drag. The click
            // fires here rather than on release so navigation stays immediate;
            // a drag that follows selects text without undoing it.
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(session) = self.harness_focus.attached_to().map(str::to_string) {
                    let inside_attached_pane =
                        self.hit_harness.as_ref().is_some_and(|(rect, id)| {
                            id == &session && rect.contains((m.column, m.row).into())
                        });
                    if !inside_attached_pane {
                        // A click that navigates away releases the same keyboard
                        // focus as Ctrl-]. Settle the configured hand-back policy
                        // before changing the selected tab or rail row; otherwise
                        // an Ask prompt would refer to a pane already hidden.
                        if !self.begin_harness_release(&session) {
                            return None;
                        }
                        self.release_harness();
                    }
                }
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

    /// Hand a pointer event to the attached harness, if it is the harness's.
    ///
    /// Returns `true` when the event was consumed, so the caller must not also
    /// route it — a click that both reached Claude Code's permission dialog and
    /// armed our drag-selection would leave a selection nobody asked for
    /// hanging over the pane the child just redrew.
    ///
    /// Three conditions, all necessary:
    ///
    /// - a harness holds the keyboard, so the operator has already said the
    ///   pane is what they are working in;
    /// - the pointer is inside *that* harness's screen, not another pane;
    /// - the child enabled mouse reporting. A harness that never asked keeps
    ///   our own drag-to-select-and-copy, which is the only way to get text out
    ///   of one — and forwarding to it would type `ESC [ < 0 ; 4 ; 9 M` into
    ///   its composer.
    ///
    /// The wheel is deliberately absent: [`scroll_at`](Self::scroll_at) already
    /// forwards notches over any harness pane, attached or not, because reading
    /// back through a harness's output should not cost a chord first.
    fn forward_harness_mouse(&mut self, m: &crossterm::event::MouseEvent) -> bool {
        let Some(session) = self.harness_focus.attached_to().map(str::to_string) else {
            return false;
        };
        let Some((rect, id)) = self.hit_harness.clone() else {
            return false;
        };
        if id != session || !rect.contains((m.column, m.row).into()) {
            return false;
        }
        let Some((button, motion)) = pointer_report(m.kind) else {
            return false;
        };
        let Some(harnesses) = self.harnesses.clone() else {
            return false;
        };
        if !harnesses.takes_mouse(&session) {
            return false;
        }
        // Pane-relative: the child believes its screen starts at its own
        // origin, and reporting our absolute position would put the event
        // somewhere else entirely on it.
        harnesses.mouse_button(&session, m.column - rect.x, m.row - rect.y, button, motion);
        true
    }

    /// Scroll whatever the pointer is over, rather than whatever has focus.
    ///
    /// A wheel event carries a position, so it should act on the list under it —
    /// scrolling the rail while the pointer sits on the transcript is the kind of
    /// thing that makes a mouse feel bolted on. Focus is left alone: the wheel
    /// looks around, it does not move where typing goes.
    pub(super) fn scroll_at(&mut self, x: u16, y: u16, up: bool) {
        // An embedded harness is a terminal, so the wheel over it belongs to it
        // — before the subpage menu, before the tab. Deliberately *not* gated on
        // being attached: reading back through a harness's output is the most
        // common thing to want from one, and making it cost a chord first would
        // be the wrapper getting in the way.
        if let Some((rect, session)) = self.hit_harness.clone() {
            if rect.contains((x, y).into()) {
                if let Some(harnesses) = self.harnesses.clone() {
                    // Pane-relative: the child believes its screen starts at its
                    // own origin, and reporting our absolute position would put
                    // the event somewhere else entirely on it.
                    harnesses.scroll(&session, x - rect.x, y - rect.y, up, SCROLL_ROWS);
                }
                return;
            }
        }
        // The subpage menu scrolls through its pages, whichever tab drew it.
        if self.hit_nav.area.contains((x, y).into()) {
            self.scroll_subpage(up);
            return;
        }
        match self.tab() {
            // Over the rail, the wheel walks the cursor over the lanes and
            // their tasks; anywhere else on the tab it scrolls the transcript.
            "Agents" => {
                let over_rail = self
                    .hit_agents
                    .as_ref()
                    .is_some_and(|(rail, _)| rail.contains((x, y).into()));
                if over_rail {
                    self.move_agent_index(up);
                } else {
                    self.scroll_transcript(up, 3);
                }
            }
            "Workflows" => {
                if self
                    .hit_workflow_preview
                    .is_some_and(|area| area.contains((x, y).into()))
                {
                    self.wf.preview_scroll = if up {
                        self.wf.preview_scroll.saturating_sub(SCROLL_ROWS)
                    } else {
                        self.wf.preview_scroll.saturating_add(SCROLL_ROWS)
                    };
                }
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
            "TokenMaxxxing" => {
                self.tokenmaxxing_index =
                    step(self.tokenmaxxing_index, TOKENMAXXING_SUBPAGES.len());
            }
            "Hosts" => self.routing_index = step(self.routing_index, ROUTING_SUBPAGES.len()),
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
                "TokenMaxxxing" => {
                    (self.tokenmaxxing_index, self.tokenmaxxing_focused) = (page, true);
                }
                "Hosts" => (self.routing_index, self.routing_focused) = (page, true),
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
            if let Some((rect, owners)) = self.hit_agents.clone() {
                if rect.contains((x, y).into()) {
                    // Each drawn line records the rail row it came from, so a
                    // click on the second line of a wrapped harness row selects
                    // that harness rather than whatever follows it. The map
                    // covers the unselectable rows too — the `── functions ──`
                    // separator and the `+N more` counter — because
                    // `agent_index` indexes all of them.
                    let rel = (y - rect.y) as usize;
                    let rows = self.rail_rows();
                    if let Some(row) = owners.get(rel).and_then(|idx| rows.get(*idx)) {
                        if row.selectable() {
                            let idx = owners[rel];
                            self.agent_scroll = 0;
                            self.chat_scroll = 0;
                            self.agent_index = idx;
                            // A click is a focus gesture: the arrows should now
                            // continue from the row that was just picked.
                            self.focus_agents_rail();
                            // The action row acts on the click that lands on it;
                            // requiring a second keystroke to confirm what was
                            // already aimed at is the friction it exists to
                            // remove.
                            if row.is_new_harness() {
                                self.open_harness_picker();
                                return None;
                            }
                            // Clicking a task is the request to watch it.
                            if let Some(cmd) = self.retarget_watch() {
                                return Some(cmd);
                            }
                        }
                    }
                }
            }
            // A click inside the embedded terminal means "type here", the same
            // as `Ctrl-]`. Checked after the rail so a click that changes rows
            // is a navigation, not an attach to whatever the last frame showed.
            if let Some((rect, session)) = self.hit_harness.clone() {
                if rect.contains((x, y).into())
                    && self.harness_pane_session.as_deref() == Some(session.as_str())
                    && !self.harness_focus.is_attached_to(&session)
                {
                    self.attach_to_pane_harness();
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
    /// Not every row can hold the cursor: the `── functions ──` separator and
    /// the `+N more` counter are labels, not destinations. So this steps over
    /// them to the next lane or task rather than stopping on one, and a cursor
    /// that would leave the list stays where it was.
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
