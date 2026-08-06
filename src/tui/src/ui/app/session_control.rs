//! Starting sessions the operator owns, and moving control between them and
//! the orchestrator.
//!
//! Two features that turn out to be one. "Unmanaged" is not a kind of session —
//! it is a session the operator holds, and dispatch skips anything the operator
//! holds. So spawning one, taking one over, and handing one back are three
//! spellings of the same state change, and they live together here.
//!
//! The state changes here are synchronous. Opening a PTY and setting a flag are
//! both immediate, so the operator gets an answer on the status line in the same
//! keystroke — which is the whole point of a control handover being *explicit*.
//!
//! Telling the *orchestrator* is not immediate, so that part is queued as a
//! [`Cmd`](super::types::Cmd) and travels off-thread. Control flips locally
//! first and the brief follows: a handback gated on a socket round-trip would
//! fail whenever the uplink is down, which is exactly when an operator most
//! wants to let go of a session.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::protocol::HarnessProvider;

use crate::ui::composer::Draft;
use crate::ui::harness_pane::HarnessChoice;
use crate::worker::pty::SessionControl;

use super::types::{
    tab_pos, AgentPicker, AgentPickerStep, App, Cmd, HandbackPolicy, HandbackPrompt, PickerPurpose,
    TakeOrigin,
};

impl App {
    /// Open the "start a session" picker, or spawn directly when the command
    /// already named a harness type.
    ///
    /// `/session` with no harness type opens the picker rather than guessing:
    /// starting the wrong CLI in the operator's workspace is not something they
    /// find out about until it has already done something.
    pub(super) fn start_session_command(&mut self, provider: Option<&str>, path: Option<&str>) {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions to start");
            return;
        };
        match provider.and_then(HarnessProvider::from_wire) {
            Some(provider) => {
                let cwd = path.unwrap_or("").to_string();
                self.spawn_session(HarnessChoice::native(provider), &cwd);
            }
            None => {
                let choices = harnesses.choices();
                if choices.is_empty() {
                    self.set_status("No harness CLIs found on this device");
                    return;
                }
                self.agent_picker = Some(AgentPicker {
                    purpose: PickerPurpose::Spawn,
                    choices,
                    index: 0,
                    step: AgentPickerStep::Harness,
                    cwd: path
                        .map(str::to_string)
                        .unwrap_or_else(|| harnesses.workspace.clone()),
                    workspace_query: String::new(),
                    workspace_choices: Vec::new(),
                    workspace_index: 0,
                    workspace_picked: false,
                });
            }
        }
    }

    /// Open the picker from the keyboard shortcut.
    pub(crate) fn open_session_picker(&mut self) {
        self.start_session_command(None, None);
    }

    /// Start a session the operator owns and move the cursor onto it.
    ///
    /// Always unmanaged, and not as a default the operator can override: a
    /// session started by hand is one somebody intends to type into, and the
    /// orchestrator starts its own managed without being asked. Spawning one
    /// into dispatch would mean the very next thing the operator does — press
    /// Enter on the row they just created — is a request to take it back off
    /// the orchestrator it was handed to a keystroke earlier.
    ///
    /// Selecting the new row matters more than it sounds: a session that
    /// appears somewhere below the fold, with the pane still showing whatever
    /// was selected before, reads as "nothing happened".
    pub(super) fn spawn_session(&mut self, choice: HarnessChoice, cwd: &str) {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions to start");
            return;
        };
        let skip = self.harness_skip_permissions;
        let workspace = harnesses.resolve_workspace(cwd);
        match harnesses.open_unmanaged(&choice, &workspace, skip) {
            Ok(id) => {
                self.tab_index = tab_pos("Agents");
                self.select_session_row(&id);
                // "unmanaged" and not a friendlier synonym: it is the word the
                // rail badge uses for the same session, and a status line that
                // renamed the state would leave the operator matching two
                // vocabularies for one fact.
                let mut status = format!(
                    "Started {} · unmanaged, the orchestrator will not use it",
                    choice.display_name(),
                );
                if let Err(error) = self.remember_harness_workspace(&workspace) {
                    status.push_str(&format!(" · {error}"));
                }
                self.set_status(status);
                // The quick path always leaves a declared agent behind if the
                // operator wants one: a session in a directory nothing declares
                // is a real thing running that the rail can only list loose.
                self.offer_agent_declaration(choice.id(), &workspace);
            }
            // Surfaced, never swallowed: a spawn that fails silently leaves the
            // operator waiting for a pane that is never coming.
            Err(err) => {
                self.set_status(format!("Could not start {}: {err}", choice.display_name()))
            }
        }
    }

    /// Take the selected session from the orchestrator.
    pub(crate) fn take_session_control(&mut self) {
        let Some((_, session)) = self.selected_session() else {
            return;
        };
        self.take_session(&session, TakeOrigin::Explicit);
    }

    /// Take one named session from the orchestrator, recording how.
    ///
    /// Named rather than cursor-resolved because most callers already know
    /// which session they mean — the prompt they are answering, the row that
    /// was clicked — and re-deriving it from the rail invites the two to
    /// disagree about which session the operator is moving.
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
            // A session that has gone is not taken. Setting control on a corpse
            // would report success and leave the operator waiting to type into
            // a pane that is never coming back.
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

    /// Give the selected session back to the orchestrator, with an optional note.
    pub(crate) fn hand_session_back(&mut self, note: Option<String>) {
        let Some((harnesses, session)) = self.handoff_target() else {
            return;
        };
        if harnesses.control(&session) == Some(SessionControl::Orchestrator) {
            self.set_status("The orchestrator already has this session");
            return;
        }
        self.hand_back_session(&session, note);
    }

    /// Toggle who holds the session `/handoff` would mean — the `Ctrl-G` shortcut.
    ///
    /// One key for both directions because the rail row and the pane title both
    /// say which way it will go, so a single "grab or give" is less to remember
    /// than two chords that each do nothing half the time.
    ///
    /// Resolved through [`handoff_target`](Self::handoff_target) rather than
    /// through the rail cursor alone, because `Ctrl-G` is a *global* chord: it
    /// fires on every tab, and on every tab but Agents the cursor resolves no
    /// session at all. That made the chord report "no session on this row" while
    /// the operator was attached to one and looking straight at it.
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

    /// Enter (or a click) on a session row: go in, asking first only if going in
    /// would take the session off the orchestrator.
    ///
    /// The two cases are not symmetric, and treating them as though they were is
    /// what made this flow confusing.
    ///
    /// A session the operator already holds has nothing to negotiate. Enter
    /// means "let me type in this", so it attaches, full stop. It used to raise
    /// the hand-back question instead — answering a request to go *in* with an
    /// offer to give the session *away*, over the pane the operator was aiming
    /// at, whose only useful answer was Escape. With operator-started sessions
    /// now unmanaged by default, that was every session on the rail.
    ///
    /// A session the orchestrator holds is the case worth a question: typing
    /// into it takes it out from under dispatch, and Enter is a navigation key —
    /// an operator walking the rail to look closer should not silently lock the
    /// orchestrator out of a workspace. `Ctrl-]` remains the deliberate spelling
    /// and still attaches outright.
    pub(crate) fn open_session_enter_prompt(&mut self) {
        let Some((harnesses, session)) = self.selected_session() else {
            return;
        };
        match harnesses.control(&session) {
            Some(SessionControl::User) => self.attach_to_pane_session(),
            // The orchestrator holds it: typing into it means taking it first.
            Some(SessionControl::Orchestrator) => {
                self.handback_prompt = Some(HandbackPrompt {
                    session,
                    took_control: false,
                    note: Draft::default(),
                    editing_note: false,
                    is_takeover: true,
                });
            }
            None => {
                self.set_status("That session is gone");
            }
        }
    }

    /// The session `/handoff` means, without depending on a render having run.
    ///
    /// [`selected_session`](Self::selected_session) reads `pane_session`,
    /// which is written inside the Agents pane's draw and cleared at the top of
    /// every frame. So `/handoff` typed from any other tab reported "no session
    /// on this row" while the operator was demonstrably holding one — and with a
    /// note argument that is worse, because they have just typed a sentence that
    /// is then thrown away.
    ///
    /// In order: the attached session (unambiguous — the keyboard is in it), the
    /// session the last frame resolved, then the single running session the
    /// operator holds. Ambiguity is reported, never guessed: handing back the
    /// wrong session puts an agent into a workspace somebody is still using.
    fn handoff_target(&mut self) -> Option<(crate::ui::harness_pane::LocalSessions, String)> {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions");
            return None;
        };
        if let Some(session) = self.harness_focus.attached_to() {
            return Some((harnesses, session.to_string()));
        }
        // A remote row is a session the cursor is genuinely on — it is just not
        // one this machine can hand over (§E7). Refused here rather than fallen
        // through, because the fallbacks below would then resolve to *something
        // else*: with the cursor on a remote row `pane_session` is empty, so the
        // single-held-session rule would hand back an unrelated local session
        // the operator was not even looking at. That is the exact "handed back
        // the wrong one" failure this function exists to prevent, and it is
        // worse than the refusal because it is silent.
        if let Some(agent) = self.pane_remote_session.clone() {
            self.set_status(format!(
                "{agent} runs on another host — you can watch this session, but \
                 taking control is local-only for now"
            ));
            return None;
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

    /// The session the cursor is on, with the handle needed to act on it.
    ///
    /// Refuses with a reason rather than silently doing nothing, for the same
    /// reason the attach chord does: an operator who pressed a key and saw no
    /// change cannot tell "wrong row" from "broken feature".
    fn selected_session(&mut self) -> Option<(crate::ui::harness_pane::LocalSessions, String)> {
        // A session on another host is a real session the cursor is really on —
        // it is just not one this machine can take (§E7). The hub resolves a
        // hold by local workspace path, so there is nothing here to flip, and
        // the honest answer names the machine rather than pretending the row is
        // empty. Watching it is unaffected: the screen mirror is read-only by
        // design either way.
        //
        // Asked before "is this device hosting", because it is the more specific
        // answer and the two are not exclusive: a laptop that hosts nothing can
        // still be looking at a remote host's session, and "this device is not
        // hosting" would be a true sentence about the wrong machine.
        if let Some(agent) = self.pane_remote_session.clone() {
            self.set_status(format!(
                "{agent} runs on another host — you can watch this session, but \
                 taking control is local-only for now"
            ));
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

    /// Decide what releasing the keyboard should do, and do it.
    ///
    /// Returns `true` when the release is settled and the caller should detach.
    /// `false` means a prompt is now open and the operator is still attached —
    /// releasing before they answer would move the keyboard out from under the
    /// question being asked about it.
    pub(crate) fn begin_session_release(&mut self, session: &str) -> bool {
        let held = self
            .local_sessions
            .as_ref()
            .and_then(|harnesses| harnesses.control(session))
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
                self.set_status(
                    "Released · you still hold this session (/handoff to give it back)",
                );
                true
            }
            // Ask about sessions the orchestrator has a claim on, not about
            // ones that were always yours. A session the operator started is
            // theirs by construction, so stepping out of it is just stepping
            // out of it — offering to hand it to an orchestrator that never had
            // it turns every `Ctrl-]` into a question with an obvious answer,
            // and questions with obvious answers get dismissed without being
            // read. `/handoff` is still there for the operator who does want to
            // give one away.
            //
            // The claim is read from three places, because no one of them is
            // sufficient — and the two that were tried alone each had a hole:
            //
            // - [`SessionOrigin`]: dispatch created the session, so walking away
            //   still user-held locks dispatch out of it. This does not depend
            //   on *how* the operator came to hold it, which matters because the
            //   executor hands a failed reusable turn straight to the operator
            //   (`worker::executor::run`) without passing through
            //   [`take_session`](Self::take_session).
            // - `orchestrator_claimed`: origin under-counts in the other
            //   direction. A session the operator started stays origin `User`
            //   forever, but once handed over it is genuinely dispatchable
            //   (`SessionHandle::serves_label` adopts it), so the same failed
            //   turn can hand it back user-held with origin `User` and no take
            //   recorded. Without this set that released in silence too.
            // - `sessions_taken`: catches a take that has not been released yet,
            //   and is what decides the *wording* below.
            HandbackPolicy::Ask => {
                let taken = self.sessions_taken.get(session).copied();
                let orchestrator_originated = self
                    .local_sessions
                    .as_ref()
                    .and_then(|sessions| sessions.sessions.row(session))
                    .is_some_and(|row| row.origin.is_orchestrator());
                let claimed = self.orchestrator_claimed.contains(session);
                if taken.is_none() && !orchestrator_originated && !claimed {
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

    /// The note typed into the handback prompt, when it is not blank.
    fn handback_note(&self) -> Option<String> {
        self.handback_prompt
            .as_ref()
            .map(|prompt| prompt.note.text.trim().to_string())
            .filter(|note| !note.is_empty())
    }

    /// Apply one keystroke to the handback prompt's note.
    ///
    /// A single line, so it uses the shared draft primitives directly rather
    /// than the full composer: there is nothing here to submit, wrap, or
    /// history-scroll.
    fn edit_handback_note(&mut self, code: KeyCode) {
        let Some(prompt) = self.handback_prompt.as_mut() else {
            return;
        };
        let draft = &mut prompt.note;
        match code {
            KeyCode::Char(c) => {
                *draft = crate::ui::composer::insert_at(&draft.text, draft.cursor, &c.to_string());
            }
            KeyCode::Backspace => {
                *draft = crate::ui::composer::delete_before(&draft.text, draft.cursor);
            }
            KeyCode::Left => draft.cursor = draft.cursor.saturating_sub(1),
            KeyCode::Right => draft.cursor = (draft.cursor + 1).min(draft.text.chars().count()),
            _ => {}
        }
    }

    /// Take a bracketed-paste payload into the hand-back note.
    ///
    /// The question itself owns the keyboard and holds no field — `y`, `n` and
    /// `E` are answers, not text — so a paste made while it is up belongs to
    /// neither the session behind it nor the composer, and is dropped. After `E`
    /// the note *is* a text input, and pasting what you were doing into the
    /// brief the orchestrator receives is exactly what the note is for.
    ///
    /// Flattened to one line and inserted at the caret, matching
    /// [`edit_handback_note`](Self::edit_handback_note): the note is drawn as a
    /// single row, and `Enter` there hands the session back rather than breaking
    /// the line.
    pub(super) fn paste_into_handback_note(&mut self, text: &str) {
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

    /// Hand `session` back and queue its brief. Every handback path ends here.
    ///
    /// The order matters. The transcript is read while the session is still
    /// ours; control flips next, so the operator gets an answer on the same
    /// keystroke; the brief is queued last and travels asynchronously.
    ///
    /// That ordering means the orchestrator can dispatch into the session before
    /// it has read the brief, and that is the right trade. The brief is
    /// *context*, not permission — gating the flip on a socket round-trip would
    /// make handing a session back fail whenever the uplink is down, which is
    /// exactly when an operator most wants to let go of one.
    pub(super) fn hand_back_session(&mut self, session: &str, note: Option<String>) {
        let Some(harnesses) = self.local_sessions.clone() else {
            return;
        };
        // Read the row first: a session that has already gone is not handed
        // back, and flipping control on a corpse would advertise a session that
        // does not exist.
        let Some(row) = harnesses.sessions.row(session) else {
            self.set_status("That session is gone");
            return;
        };
        let lines = harnesses
            .sessions
            .tail_lines(session, medulla::hub::handoff::TRANSCRIPT_LINES);

        harnesses.set_control(session, SessionControl::Orchestrator);
        // Only this session's take is settled. Clearing the whole map here is
        // what a single flag amounted to, and it silently forgave every *other*
        // session the operator was still holding on the orchestrator's behalf.
        self.sessions_taken.remove(session);
        // Remembered past the handback, and never cleared: from here the
        // orchestrator can dispatch into this session, so if it ever comes back
        // to the operator — including by the executor handing a failed turn over
        // directly — letting go of it again is worth a question.
        self.orchestrator_claimed.insert(session.to_string());

        let brief = medulla::hub::handoff::normalize(
            medulla::hub::HarnessHandoff {
                // Per handback *event*, not per session: a second handback of the
                // same session is new work, and reusing the id would have the
                // orchestrator ignore it as something it already picked up.
                id: format!("{}-{}", row.id, medulla::clock::now_millis()),
                at: medulla::clock::now_millis(),
                session_id: row.id.clone(),
                harness_session_id: row.session_id.clone(),
                provider: row.provider.as_str().to_string(),
                workspace_path: row.cwd.clone(),
                // Filled off-thread: reading them shells out to git.
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
}

impl App {
    /// Route a key while the "start a session" picker is open.
    ///
    /// The first step chooses a registered harness type. The second step owns text
    /// input directly so filtering and filesystem completion update as the
    /// operator types.
    pub(super) fn handle_agent_picker_key(&mut self, event: KeyEvent) {
        let code = event.code;
        let step = self
            .agent_picker
            .as_ref()
            .map(|picker| picker.step)
            .unwrap_or(AgentPickerStep::Harness);
        if step == AgentPickerStep::Workspace {
            self.handle_harness_workspace_key(event);
            return;
        }
        match code {
            KeyCode::Esc => {
                self.agent_picker = None;
                self.set_status("Cancelled");
            }
            KeyCode::Up => {
                if let Some(picker) = &mut self.agent_picker {
                    picker.index = picker.index.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.agent_picker {
                    picker.index = (picker.index + 1).min(picker.choices.len().saturating_sub(1));
                }
            }
            KeyCode::Char('e') if is_text_input(event.modifiers) => {
                self.open_harness_workspace_step(true);
            }
            KeyCode::Enter => {
                self.open_harness_workspace_step(false);
            }
            _ => {}
        }
    }

    /// Route a key while choosing and completing the workspace directory.
    fn handle_harness_workspace_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc | KeyCode::BackTab => {
                if let Some(picker) = &mut self.agent_picker {
                    picker.step = AgentPickerStep::Harness;
                }
                self.set_status("Pick a harness type · Enter workspace · Esc cancel");
            }
            // Moving the cursor is the operator choosing a completion over
            // whatever they entered, however few rows there are to move across.
            KeyCode::Up => {
                if let Some(picker) = &mut self.agent_picker {
                    picker.workspace_index = picker.workspace_index.saturating_sub(1);
                    picker.workspace_picked = !picker.workspace_choices.is_empty();
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.agent_picker {
                    picker.workspace_index = (picker.workspace_index + 1)
                        .min(picker.workspace_choices.len().saturating_sub(1));
                    picker.workspace_picked = !picker.workspace_choices.is_empty();
                }
            }
            KeyCode::Tab => self.complete_harness_workspace(),
            KeyCode::Backspace => {
                if let Some(picker) = &mut self.agent_picker {
                    picker.workspace_query.pop();
                    picker.workspace_index = 0;
                    picker.workspace_picked = false;
                }
                self.refresh_harness_workspace_choices();
            }
            KeyCode::Char(character) if is_text_input(event.modifiers) => {
                if let Some(picker) = &mut self.agent_picker {
                    picker.workspace_query.push(character);
                    picker.workspace_index = 0;
                    picker.workspace_picked = false;
                }
                self.refresh_harness_workspace_choices();
            }
            // The last step, and it says so in its own title: Enter finishes the
            // picker. It used to lead to a control question instead, which the
            // hint text never mentioned and which had one sensible answer.
            KeyCode::Enter => {
                let Some(workspace) = self.selected_picker_workspace() else {
                    self.set_status("Choose an existing directory");
                    return;
                };
                let purpose = self
                    .agent_picker
                    .as_ref()
                    .map(|picker| picker.purpose.clone())
                    .unwrap_or(PickerPurpose::Spawn);
                let Some(choice) = self
                    .agent_picker
                    .as_ref()
                    .and_then(|picker| picker.choices.get(picker.index).cloned())
                else {
                    self.set_status("Choose a harness type first");
                    return;
                };
                self.agent_picker = None;
                // Declaring is finished by naming, not by starting: nothing runs
                // yet, so there is nobody to hand control to.
                if purpose == PickerPurpose::DeclareAgent {
                    let harness = choice.id().to_string();
                    self.prompt_agent_name(&harness, &workspace);
                    return;
                }
                self.spawn_session(choice, &workspace);
            }
            _ => {}
        }
    }

    /// Route a key while the hand-back question is open.
    ///
    /// Enter means yes, because handing back is the safe answer: a session left
    /// under a user who has walked away is one the orchestrator can never use,
    /// and that failure is silent.
    pub(super) fn handle_handback_key(&mut self, code: KeyCode) {
        let Some(prompt) = self.handback_prompt.as_ref() else {
            return;
        };
        // The takeover direction has no note and nothing to release: the
        // operator is not holding the session yet, so the only two answers are
        // "take it and start typing" and "leave it alone".
        if prompt.is_takeover {
            // The prompt's own session, not the rail's. The two agree today
            // because the question owns the keyboard and the pointer while it is
            // up, but resolving through the cursor makes "which session does `y`
            // move?" depend on what the last frame happened to draw — and the
            // failure it invites is moving control of a session the operator
            // never pointed at.
            let session = prompt.session.clone();
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.handback_prompt = None;
                    self.take_session(&session, TakeOrigin::Explicit);
                    self.attach_to_session(&session);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.handback_prompt = None;
                    self.set_status("Cancelled");
                }
                _ => {}
            }
            return;
        }
        let session = prompt.session.clone();
        // While the note is being typed, every key is text. `y` and `n` have to
        // keep meaning yes and no, so an operator whose note begins "no, the
        // migration…" must not have the first letter answer for them.
        if prompt.editing_note {
            match code {
                KeyCode::Enter => {
                    let note = self.handback_note();
                    self.handback_prompt = None;
                    self.hand_back_session(&session, note);
                    self.release_session();
                }
                // Back to the question, keeping what was typed: an operator who
                // pressed Escape meant "stop typing", not "discard my sentence".
                KeyCode::Esc => {
                    if let Some(prompt) = self.handback_prompt.as_mut() {
                        prompt.editing_note = false;
                    }
                }
                _ => self.edit_handback_note(code),
            }
            return;
        }
        match code {
            KeyCode::Char('e') | KeyCode::Char('E') => {
                if let Some(prompt) = self.handback_prompt.as_mut() {
                    prompt.editing_note = true;
                }
            }
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let note = self.handback_note();
                self.handback_prompt = None;
                self.hand_back_session(&session, note);
                self.release_session();
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.handback_prompt = None;
                self.release_session();
                self.set_status(
                    "Released · you still hold this session (/handoff to give it back)",
                );
            }
            // Esc is "I did not mean to leave", so it puts the operator back
            // where they were rather than picking one of the answers for them.
            KeyCode::Esc => {
                self.handback_prompt = None;
                self.set_status("Still typing into the session");
            }
            _ => {}
        }
    }
}

/// Only printable text belongs in the workspace query; modifier chords must
/// never be mistaken for their underlying character.
pub(super) fn is_text_input(modifiers: KeyModifiers) -> bool {
    modifiers == KeyModifiers::NONE
        || modifiers == KeyModifiers::SHIFT
        || modifiers == (KeyModifiers::CONTROL | KeyModifiers::ALT)
}
