//! Pointer input for [`App`]: wheel scrolling, click hit-testing, drag
//! selection, and the reports forwarded to an embedded harness.
//!
//! Split from [`super`] because the pointer answers a different question from
//! the keyboard: a mouse event carries a position, so nearly everything here is
//! resolved by *what is under it* rather than by what has focus.

use crossterm::event::{MouseButton, MouseEventKind};

use super::super::types::{App, Cmd, ROUTING_SUBPAGES, SETTINGS_SUBPAGES, TOKENMAXXING_SUBPAGES};

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
    /// Handle scroll and left-click mouse events for the active tab.
    pub(in crate::ui::app) fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Option<Cmd> {
        if self.kill_armed.take().is_some() {
            self.set_status("Harness kill cancelled");
        }
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
    pub(in crate::ui::app) fn scroll_at(&mut self, x: u16, y: u16, up: bool) {
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
    pub(in crate::ui::app) fn extend_selection(&mut self, x: u16, y: u16) {
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
    pub(in crate::ui::app) fn handle_click(&mut self, x: u16, y: u16) -> Option<Cmd> {
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
}
