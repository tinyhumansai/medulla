//! Pointer input for [`App`]: wheel scrolling, click hit-testing, drag
//! selection, and the reports forwarded to an embedded harness.
//!
//! Split from [`super`] because the pointer answers a different question from
//! the keyboard: a mouse event carries a position, so nearly everything here is
//! resolved by *what is under it* rather than by what has focus.

use crossterm::event::{MouseButton, MouseEventKind};

use super::super::types::{
    App, Cmd, PointerGrab, ROUTING_SUBPAGES, SETTINGS_SUBPAGES, TOKENMAXXING_SUBPAGES,
};

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
        let kill_cancelled = self.kill_armed.take().is_some();
        if kill_cancelled {
            self.set_status("Session kill cancelled");
        }
        let harness_close_cancelled = self.harness_close_armed.take().is_some();
        if harness_close_cancelled {
            self.set_status("Harness close cancelled");
        }
        let workflow_delete_cancelled = self.workflow_delete_armed.take().is_some();
        if workflow_delete_cancelled {
            self.set_status("Workflow deletion cancelled");
        }
        // The cancellation click is the modal's answer, not a second click on
        // the content it covered. Returning here also prevents a captured
        // harness pointer gesture from receiving a stray release after a
        // destructive confirmation is dismissed.
        if kill_cancelled || harness_close_cancelled || workflow_delete_cancelled {
            return None;
        }
        // Ahead of every other rule, including the modal one below: a button
        // that went down in a harness has to come back up in it. The grab is
        // the whole gesture, so it outranks whatever the pointer has since
        // moved over and whatever the press itself put on screen.
        if self.deliver_pointer_grab(&m) {
            return None;
        }
        // The hand-back question's answers are click targets. It is the one
        // overlay a click can raise, so it is the one an operator arrives at
        // with their hand already on the mouse — being made to reach for the
        // keyboard to answer a question the pointer just asked is the friction
        // that makes the modal feel broken.
        if self.handback_prompt.is_some() {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                if let Some(key) = self
                    .hit_handback
                    .iter()
                    .find(|(rect, _)| rect.contains((m.column, m.row).into()))
                    .map(|(_, key)| *key)
                {
                    self.handle_handback_key(key);
                    // Returned from here rather than falling through: answering
                    // can close the question, and the swallow below would then
                    // see no modal and let the very same click carry on into
                    // the rail underneath it.
                    return None;
                }
            }
            // Everything else the question is over is still swallowed, below.
        }
        // The inline prompt owns the pointer over the picker exactly as it owns
        // the keyboard (see `on_key`, which routes the prompt before the picker):
        // it is an edit on top of a modal, and a click that replayed Enter on a
        // picker row behind it would start a harness while the favorite-name
        // prompt was still on screen. The prompt itself is a text entry with no
        // click targets, so every pointer event is swallowed while it is open.
        if self.prompt.is_some() {
            return None;
        }
        // The picker's rows are click targets too, and its list takes the wheel.
        // Same reasoning as the question above: it is opened from a rail row the
        // operator clicked (`+ New session`) or from `Ctrl-T`, so they arrive
        // with a hand on the mouse — and a modal that then refuses the pointer
        // entirely reads as a frozen screen rather than as a keyboard-only step.
        if self.session_picker.is_some() && self.route_session_picker_pointer(&m) {
            return None;
        }
        // A modal swallows the mouse, the same way it swallows the keyboard.
        // Pickers and the hand-back question are modal: a click that navigated
        // the rail behind one would leave an overlay describing a row nobody
        // was pointing at. In particular, do not let a second session click
        // replace the session named by an already-visible hand-back prompt.
        if self.workflow_delete_armed.is_some() {
            return None;
        }
        if self.resume_picker.is_some()
            || self.session_picker.is_some()
            || self.kill_armed.is_some()
            || self.harness_close_armed.is_some()
            || self.handback_prompt.is_some()
        {
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
                        self.hit_session.as_ref().is_some_and(|(rect, id)| {
                            id == &session && rect.contains((m.column, m.row).into())
                        });
                    // Only the attached harness's own rail row is not a
                    // destination away from it: clicking the row you are
                    // already typing in should move nothing and ask nothing.
                    // Every other row is a departure, and used not to be
                    // treated as one — the rail was waved through wholesale, so
                    // a click on the lane next door skipped the hand-back
                    // policy here and was then released silently by the next
                    // draw, which notices the cursor has left the attached
                    // session. Worse, a click on *another harness's* row opened
                    // the handover question about that harness while detaching
                    // this one, so answering it handed back a session the
                    // operator had never typed in and left the one they had
                    // held forever.
                    let on_own_rail_row =
                        self.rail_session_at(m.column, m.row).as_deref() == Some(session.as_str());
                    if !inside_attached_pane && !on_own_rail_row {
                        // A click that navigates away releases the same keyboard
                        // focus as Ctrl-]. Settle the configured hand-back policy
                        // before changing the selected tab or rail row; otherwise
                        // an Ask prompt would refer to a pane already hidden.
                        if !self.begin_session_release(&session) {
                            return None;
                        }
                        self.release_session();
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

    /// Route a pointer event that belongs to the open "start a session" picker.
    ///
    /// Returns `true` when the event was consumed. Everything else over the
    /// picker falls through to the blanket modal swallow — including a click
    /// outside its box, which deliberately does *not* cancel: the workspace step
    /// holds a query the operator has typed, and losing it to a stray click a
    /// few cells wide of the border is worse than a click that does nothing.
    ///
    /// Both gestures replay the keystroke they stand for rather than editing the
    /// picker themselves. A click is Enter on the row it lands on and a notch is
    /// an arrow, so the pointer cannot come to disagree with the keyboard about
    /// what selecting a row does — and the workspace step's Enter *starts a
    /// session*, which is exactly the divergence worth designing out.
    fn route_session_picker_pointer(&mut self, m: &crossterm::event::MouseEvent) -> bool {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let Some((area, rows)) = self.hit_session_picker.clone() else {
            return false;
        };
        let at = (m.column, m.row).into();
        if !area.contains(at) {
            return false;
        }
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.handle_session_picker_key(key(KeyCode::Up));
                true
            }
            MouseEventKind::ScrollDown => {
                self.handle_session_picker_key(key(KeyCode::Down));
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(index) = rows
                    .iter()
                    .find(|(rect, _)| rect.contains(at))
                    .map(|(_, index)| *index)
                else {
                    // Inside the box but not on a row — a title, a hint, the
                    // search line. Consumed so it cannot reach the rail behind,
                    // but it selects nothing.
                    return true;
                };
                let Some(picker) = self.session_picker.as_mut() else {
                    return true;
                };
                match picker.step {
                    super::super::types::SessionPickerStep::Harness => picker.index = index,
                    super::super::types::SessionPickerStep::Workspace => {
                        picker.workspace_index = index;
                        // The click *is* the operator choosing this completion
                        // over whatever they had typed, which is precisely what
                        // this flag records for `selected_picker_workspace`.
                        picker.workspace_picked = true;
                    }
                }
                self.handle_session_picker_key(key(KeyCode::Enter));
                true
            }
            _ => false,
        }
    }

    /// The harness session named by the rail row under `(x, y)`, if any.
    ///
    /// Resolved through the same drawn-line-to-row map [`handle_click`] uses,
    /// so a click on the second line of a wrapped harness row answers with that
    /// harness rather than with whatever follows it.
    fn rail_session_at(&self, x: u16, y: u16) -> Option<String> {
        let (rect, owners) = self.hit_agents.as_ref()?;
        if !rect.contains((x, y).into()) {
            return None;
        }
        owners.get((y - rect.y) as usize)?.target.session_id()
    }

    /// Deliver a drag or release to the harness that took the matching press.
    ///
    /// Returns `true` when the event belonged to a grab and was consumed. Only
    /// the grabbed button's own motion is claimed: a second button pressed
    /// meanwhile, or the wheel, still routes normally.
    ///
    /// The coordinates are clamped into the pane rather than dropped when the
    /// pointer has left it. That is what a terminal reports for a drag off the
    /// edge of a window, and it is the only encoding that keeps the child's idea
    /// of where the pointer is inside the screen it believes it owns — a release
    /// at a negative offset would wrap to the far side of the pane instead.
    fn deliver_pointer_grab(&mut self, m: &crossterm::event::MouseEvent) -> bool {
        let Some(grab) = self.pointer_grab.clone() else {
            return false;
        };
        let Some((button, motion)) = pointer_report(m.kind) else {
            return false;
        };
        use crate::ui::harness_pane::mouse::Motion;
        // A press never belongs to a grab: it starts one. Leaving it to the
        // ordinary path also means a press elsewhere while a button is held
        // behaves exactly as it does with no harness involved.
        if button != grab.button || motion == Motion::Press {
            return false;
        }
        if motion == Motion::Release {
            self.pointer_grab = None;
        }
        let Some(harnesses) = self.local_sessions.clone() else {
            return true;
        };
        let rect = grab.rect;
        let col = m
            .column
            .clamp(rect.x, rect.right().saturating_sub(1))
            .saturating_sub(rect.x);
        let row = m
            .row
            .clamp(rect.y, rect.bottom().saturating_sub(1))
            .saturating_sub(rect.y);
        harnesses.mouse_button(&grab.session, col, row, button, motion);
        true
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
        let Some((rect, id)) = self.hit_session.clone() else {
            return false;
        };
        if id != session || !rect.contains((m.column, m.row).into()) {
            return false;
        }
        let Some((button, motion)) = pointer_report(m.kind) else {
            return false;
        };
        let Some(harnesses) = self.local_sessions.clone() else {
            return false;
        };
        if !harnesses.takes_mouse(&session) {
            return false;
        }
        // Pane-relative: the child believes its screen starts at its own
        // origin, and reporting our absolute position would put the event
        // somewhere else entirely on it.
        harnesses.mouse_button(&session, m.column - rect.x, m.row - rect.y, button, motion);
        // The press opens the grab that owns the rest of the gesture. Recorded
        // even for a child in press-only mode, whose release
        // [`mouse_button`](crate::ui::harness_pane::LocalSessions::mouse_button)
        // will drop: the grab is about *routing*, and a release routed to the
        // harness and then dropped by its protocol is right, while the same
        // release re-routed to our own drag-selection is not.
        match motion {
            crate::ui::harness_pane::mouse::Motion::Press => {
                self.pointer_grab = Some(PointerGrab {
                    session,
                    button,
                    rect,
                });
            }
            crate::ui::harness_pane::mouse::Motion::Release => {
                self.pointer_grab = None;
            }
            crate::ui::harness_pane::mouse::Motion::Drag => {}
        }
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
        if let Some((rect, session)) = self.hit_session.clone() {
            if rect.contains((x, y).into()) {
                if let Some(harnesses) = self.local_sessions.clone() {
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
            "Sessions" => {
                let over_rail = self
                    .hit_agents
                    .as_ref()
                    .is_some_and(|(rail, _)| rail.contains((x, y).into()));
                if over_rail {
                    self.move_rail_index(up);
                } else {
                    #[cfg(feature = "workflows")]
                    if self
                        .hit_workflow_preview
                        .is_some_and(|area| area.contains((x, y).into()))
                    {
                        self.wf.preview_scroll = if up {
                            self.wf.preview_scroll.saturating_sub(SCROLL_ROWS)
                        } else {
                            self.wf.preview_scroll.saturating_add(SCROLL_ROWS)
                        };
                    } else {
                        self.scroll_transcript(up, 3);
                    }
                    #[cfg(not(feature = "workflows"))]
                    self.scroll_transcript(up, 3);
                }
            }
            #[cfg(feature = "workflows")]
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
        if tab == "Sessions" {
            // §A7: clicking an entry of the orchestrator's "sessions started"
            // block opens that session — the rail selection follows, which is
            // what makes the pane show its conversation. Checked before the rail
            // because the block lives in the pane beside it.
            if let Some((rect, tasks)) = self.hit_started_sessions.clone() {
                if rect.contains((x, y).into()) {
                    // Only the rows that *are* entries answer; a click on the
                    // conversation between them falls through to the rail's own
                    // hit test, exactly as a click above the block used to.
                    if let Some(task_id) = tasks.get((y - rect.y) as usize).and_then(Option::as_ref)
                    {
                        let task_id = task_id.clone();
                        self.focus_session_for_task(&task_id);
                        return self.retarget_watch();
                    }
                }
            }
            // The rail stacks two hit boxes — threads above lanes — so both are
            // tried; an `else if` here would leave the strip unclickable.
            if let Some((rect, window_start)) = self.hit_threads {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    let idx = window_start + rel;
                    if let Some(t) = self.snapshot.threads.get(idx) {
                        let id = t.id.clone();
                        self.runtime.set_active_thread(id);
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
                    // separator — because `rail_index` indexes all of them.
                    let rel = (y - rect.y) as usize;
                    if let Some(hit) = owners.get(rel) {
                        if hit.selectable() {
                            // The hit map describes the frame the operator saw,
                            // but actions below read the current rail. A row
                            // that vanished must not use its old numeric offset
                            // as a substitute target: that can page or watch a
                            // different row that happened to move into place.
                            let lanes = self.lanes();
                            let rows = self.rail_rows_in(&lanes);
                            if !hit.exists_in(&rows, &lanes) {
                                return None;
                            }
                            self.agent_scroll = 0;
                            self.set_rendered_rail_cursor(hit);
                            #[cfg(feature = "workflows")]
                            self.sync_selected_workflow_run();
                            // The action row acts on the click that lands on it;
                            // requiring a second keystroke to confirm what was
                            // already aimed at is the friction it exists to
                            // remove.
                            //
                            // Both action branches return the retarget rather
                            // than nothing, for the same reason the overflow and
                            // harness branches below do: an action row watches
                            // no task, so a click arriving from one that did has
                            // to stop that stream — and neither open method
                            // clears `watching` on its own.
                            if matches!(&hit.target, super::super::types::RailHitTarget::NewSession)
                            {
                                self.open_session_picker();
                                return self.retarget_watch();
                            }
                            if let super::super::types::RailHitTarget::WorkflowRun {
                                workflow_id,
                                run_id,
                            } = &hit.target
                            {
                                self.open_workflow_run(workflow_id, run_id);
                                return self.retarget_watch();
                            }
                            // So is a lane's `+N more`: the click that lands on
                            // it is the request to see what it is counting.
                            //
                            // Returns the retarget rather than nothing, for the
                            // same reason the harness branch below does: the
                            // overflow row watches no task, so a click arriving
                            // from one has to stop that stream. The keyboard
                            // path is already covered — the arrow that reaches
                            // this row retargets on the way.
                            if matches!(&hit.target, super::super::types::RailHitTarget::Overflow)
                                && self.page_subtasks()
                            {
                                return self.retarget_watch();
                            }
                            if let Some(session) = hit.target.session_id() {
                                // Clicking the row of the harness the keyboard
                                // is already in is not a handover request. It
                                // used to raise "you still have this harness"
                                // over the pane the operator was mid-sentence
                                // in, whose only useful answer was Esc.
                                if self.harness_focus.is_attached_to(&session) {
                                    self.pane_session = Some(session.to_string());
                                    return None;
                                }
                                // Point the prompt at the row that was clicked,
                                // not at whatever the last render left behind.
                                // `pane_session` is written during the
                                // draw, and no draw happens between the cursor
                                // move above and this call — so without this the
                                // prompt would offer to hand over the previously
                                // visible harness, and confirming it would
                                // transfer control of one the operator never
                                // pointed at.
                                self.pane_session = Some(session.to_string());
                                self.open_session_enter_prompt();
                                // Drop whatever task the previous row was
                                // watching, exactly as the fall-through below
                                // does for every other row. This branch returns
                                // early, so without retargeting here a click
                                // onto a harness leaves the last task's screen
                                // streaming into a pane nobody is looking at.
                                return self.retarget_watch();
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
            // as Enter on its rail row. Checked after the rail so a click that
            // changes rows is a navigation, not an attach to whatever the last
            // frame showed.
            //
            // Routed through the same entry point Enter uses rather than
            // attaching outright, because the pointer and the keyboard must not
            // disagree about who ends up holding a session. Clicking straight
            // into an orchestrator-held pane used to take it silently — the very
            // thing the Enter question exists to prevent, reachable by the
            // gesture an operator is most likely to make first.
            if let Some((rect, session)) = self.hit_session.clone() {
                if rect.contains((x, y).into())
                    && self.pane_session.as_deref() == Some(session.as_str())
                    && !self.harness_focus.is_attached_to(&session)
                {
                    self.open_session_enter_prompt();
                }
            }
        } else if tab == "Settings" && self.settings_subpage() == "Context" {
            // Context is a Settings subpage, not a tab — matching on `tab` here
            // made this branch unreachable, so clicking a chunk did nothing.
            if let Some(rect) = self.hit_context {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    // The list is windowed on the selection (see `draw_context`),
                    // so a clicked row names `start + rel`, not `rel`. Recomputed
                    // from the same inputs the draw used — the rect it recorded
                    // and the selection, neither of which can have moved since.
                    let vis = (rect.height as usize).max(1);
                    let start = crate::ui::selection::viewport_start(
                        crate::ui::selection::clamp(self.context_index, self.contexts.len()),
                        self.contexts.len(),
                        vis,
                    );
                    let clicked = start + rel;
                    if rel < vis && clicked < self.contexts.len() {
                        self.context_index = clicked;
                    }
                }
            }
        }
        None
    }
}
