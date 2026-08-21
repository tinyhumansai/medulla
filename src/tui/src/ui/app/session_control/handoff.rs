//! Moving local harness control between the operator and orchestrator.

use crate::ui::composer::Draft;
use crate::worker::pty::SessionControl;
use crossterm::event::KeyCode;

use super::super::types::{App, Cmd, HandbackPolicy, HandbackPrompt, TakeOrigin};

impl App {
    /// Take one named session from the orchestrator and record why.
    pub(super) fn take_session(&mut self, session: &str, origin: TakeOrigin) {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions");
            return;
        };
        match harnesses.control(session) {
            Some(SessionControl::User) => {
                self.set_status("You already have this session");
                return;
            }
            None => {
                self.set_status("That session is gone");
                return;
            }
            Some(SessionControl::Orchestrator) => {}
        }
        harnesses.set_control(session, SessionControl::User);
        self.sessions_taken.insert(session.to_string(), origin);
        if let Some(cwd) = harnesses.sessions.row(session).map(|row| row.cwd) {
            self.pending_cmds.push_back(Cmd::HoldSession {
                workspace: cwd,
                reason: None,
            });
        }
        self.set_status("You have this session · the orchestrator will not dispatch into it");
    }

    /// Toggle the owner of the session selected by the handoff command.
    pub(crate) fn toggle_session_control(&mut self) {
        let Some((harnesses, session)) = self.handoff_target() else {
            return;
        };
        match harnesses.control(&session) {
            Some(SessionControl::User) => self.hand_back_session(&session, None),
            Some(SessionControl::Orchestrator) => self.take_session(&session, TakeOrigin::Explicit),
            None => self.set_status("That session is gone"),
        }
    }

    /// Attach to an operator-held session or ask before taking one over.
    pub(crate) fn open_session_enter_prompt(&mut self) {
        let Some((harnesses, session)) = self.selected_session() else {
            return;
        };
        match harnesses.control(&session) {
            Some(SessionControl::User) => self.attach_to_pane_session(),
            Some(SessionControl::Orchestrator) => {
                self.handback_prompt = Some(HandbackPrompt {
                    session,
                    took_control: false,
                    note: Draft::default(),
                    editing_note: false,
                    is_takeover: true,
                })
            }
            None => self.set_status("That session is gone"),
        }
    }

    /// Resolve the session a global handoff command applies to.
    fn handoff_target(&mut self) -> Option<(crate::ui::harness_pane::LocalSessions, String)> {
        // Ahead of the hosting check, and deliberately: a cursor on a session
        // that runs elsewhere is the more specific fact, and it is true whether
        // or not this device hosts anything of its own. Answering "not hosting"
        // to an operator plainly looking at a running session reads as a broken
        // feature rather than as the boundary it is.
        if let Some(agent) = self.pane_remote_session.clone() {
            self.set_status(format!("{agent} runs on another host — you can watch this session, but taking control is local-only for now"));
            return None;
        }
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions");
            return None;
        };
        if let Some(session) = self.harness_focus.attached_to() {
            return Some((harnesses, session.to_string()));
        }
        if let Some(session) = self.pane_session.clone() {
            return Some((harnesses, session));
        }
        let held: Vec<String> = harnesses
            .sessions
            .rows()
            .into_iter()
            .filter(|row| row.control == SessionControl::User && row.state.is_running())
            .map(|row| row.id)
            .collect();
        match held.len() {
            0 => {
                self.set_status("You are not holding any session");
                None
            }
            1 => Some((harnesses, held[0].clone())),
            n => {
                self.set_status(format!(
                    "You hold {n} sessions — select one in Agents and press Ctrl-G"
                ));
                None
            }
        }
    }

    /// Resolve a local session under the currently rendered pane.
    fn selected_session(&mut self) -> Option<(crate::ui::harness_pane::LocalSessions, String)> {
        if let Some(agent) = self.pane_remote_session.clone() {
            self.set_status(format!("{agent} runs on another host — you can watch this session, but taking control is local-only for now"));
            return None;
        }
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions");
            return None;
        };
        let Some(session) = self.pane_session.clone() else {
            self.set_status("No session on this row — select one to hand it over");
            return None;
        };
        Some((harnesses, session))
    }

    /// Decide whether releasing an attached session needs a handback prompt.
    pub(crate) fn begin_session_release(&mut self, session: &str) -> bool {
        let held = self
            .local_sessions
            .as_ref()
            .and_then(|h| h.control(session))
            == Some(SessionControl::User);
        if !held {
            return true;
        }
        match self.handback_policy {
            HandbackPolicy::Always => {
                self.hand_back_session(session, None);
                true
            }
            HandbackPolicy::Never => {
                self.set_status("Released · you still hold this session (^G gives it back)");
                true
            }
            HandbackPolicy::Ask => {
                let taken = self.sessions_taken.get(session).copied();
                let orchestrator_originated = self
                    .local_sessions
                    .as_ref()
                    .and_then(|s| s.sessions.row(session))
                    .is_some_and(|row| row.origin.is_orchestrator());
                if taken.is_none()
                    && !orchestrator_originated
                    && !self.orchestrator_claimed.contains(session)
                {
                    return true;
                }
                self.handback_prompt = Some(HandbackPrompt {
                    session: session.to_string(),
                    took_control: taken == Some(TakeOrigin::Focus),
                    note: Draft::default(),
                    editing_note: false,
                    is_takeover: false,
                });
                false
            }
        }
    }

    fn handback_note(&self) -> Option<String> {
        self.handback_prompt
            .as_ref()
            .map(|p| p.note.text.trim().to_string())
            .filter(|n| !n.is_empty())
    }

    fn edit_handback_note(&mut self, code: KeyCode) {
        let Some(prompt) = self.handback_prompt.as_mut() else {
            return;
        };
        let draft = &mut prompt.note;
        match code {
            KeyCode::Char(c) => {
                *draft = crate::ui::composer::insert_at(&draft.text, draft.cursor, &c.to_string())
            }
            KeyCode::Backspace => {
                *draft = crate::ui::composer::delete_before(&draft.text, draft.cursor)
            }
            KeyCode::Left => draft.cursor = draft.cursor.saturating_sub(1),
            KeyCode::Right => draft.cursor = (draft.cursor + 1).min(draft.text.chars().count()),
            _ => {}
        }
    }

    /// Paste a one-line note into an active handback-note editor.
    pub(crate) fn paste_into_handback_note(&mut self, text: &str) {
        let Some(prompt) = self.handback_prompt.as_mut() else {
            return;
        };
        if !prompt.editing_note {
            return;
        }
        let draft = &mut prompt.note;
        *draft = crate::ui::composer::insert_at(
            &draft.text,
            draft.cursor,
            &crate::ui::composer::flatten_paste(text),
        );
    }

    /// Return a session to the orchestrator and queue its handoff brief.
    pub(crate) fn hand_back_session(&mut self, session: &str, note: Option<String>) {
        let Some(harnesses) = self.local_sessions.clone() else {
            return;
        };
        let Some(row) = harnesses.sessions.row(session) else {
            self.set_status("That session is gone");
            return;
        };
        // A shell has nothing to hand over. There is no prompt to dispatch into
        // it and no transcript to brief anyone from, so handing one back would
        // leave a row advertised as orchestrator-held that no task can ever
        // reach — and would send a handoff brief summarising a terminal.
        if row.provider == medulla::protocol::HarnessProvider::Shell {
            self.set_status(
                "A shell session is yours — there is nothing to hand to the orchestrator",
            );
            return;
        }
        let lines = harnesses
            .sessions
            .tail_lines(session, medulla::hub::handoff::TRANSCRIPT_LINES);
        harnesses.set_control(session, SessionControl::Orchestrator);
        self.sessions_taken.remove(session);
        self.orchestrator_claimed.insert(session.to_string());
        let brief = medulla::hub::handoff::normalize(
            medulla::hub::HarnessHandoff {
                id: format!("{}-{}", row.id, medulla::clock::now_millis()),
                at: medulla::clock::now_millis(),
                session_id: row.id.clone(),
                harness_session_id: row.session_id.clone(),
                provider: row.provider.as_str().to_string(),
                workspace_path: row.cwd.clone(),
                branch: None,
                project: None,
                note,
                transcript: String::new(),
                transcript_truncated: false,
            },
            &lines,
        );
        self.pending_cmds
            .push_back(Cmd::HandOffSession(Box::new(brief)));
        self.set_status("Handed back · sending the orchestrator your brief");
    }

    /// Route a key while the handback confirmation is open.
    pub(crate) fn handle_handback_key(&mut self, code: KeyCode) {
        let Some(prompt) = self.handback_prompt.as_ref() else {
            return;
        };
        if prompt.is_takeover {
            let session = prompt.session.clone();
            match code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    self.handback_prompt = None;
                    self.take_session(&session, TakeOrigin::Explicit);
                    self.attach_to_session(&session);
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    self.handback_prompt = None;
                    self.set_status("Cancelled");
                }
                _ => {}
            }
            return;
        }
        let session = prompt.session.clone();
        if prompt.editing_note {
            match code {
                KeyCode::Enter => {
                    let note = self.handback_note();
                    self.handback_prompt = None;
                    self.hand_back_session(&session, note);
                    self.release_session();
                }
                KeyCode::Esc => {
                    if let Some(p) = self.handback_prompt.as_mut() {
                        p.editing_note = false;
                    }
                }
                _ => self.edit_handback_note(code),
            }
            return;
        }
        match code {
            KeyCode::Char('e' | 'E') => {
                if let Some(p) = self.handback_prompt.as_mut() {
                    p.editing_note = true;
                }
            }
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                let note = self.handback_note();
                self.handback_prompt = None;
                self.hand_back_session(&session, note);
                self.release_session();
            }
            KeyCode::Char('n' | 'N') => {
                self.handback_prompt = None;
                self.release_session();
                self.set_status("Released · you still hold this session (^G gives it back)");
            }
            KeyCode::Esc => {
                self.handback_prompt = None;
                self.set_status("Still typing into the session");
            }
            _ => {}
        }
    }
}
