//! The copilot thread beside the graph.
//!
//! One conversation per workflow, held in the app rather than the SDK because it
//! is screen state: what the operator has asked *this session*, and whether a
//! turn is in flight. The turn itself runs off-thread
//! ([`medulla::workflows::CopilotSession`]) and reports back through the event
//! loop, so everything here is bookkeeping around that.

use medulla::ui::workflows::CopilotState;
use medulla::workflows::copilot::{Thread, Transcripts};

use super::super::types::{App, Cmd};

/// The saved-transcript thread a pane key addresses.
///
/// [`NEW_THREAD`] is this crate's in-memory sentinel for "the workflow being
/// built has no id yet"; on disk that is a namespace of its own rather than a
/// key, so the two cannot collide with a workflow an operator actually named.
pub fn thread_of(key: &str) -> Thread<'_> {
    if key == NEW_THREAD {
        Thread::Pending
    } else {
        Thread::Workflow(key)
    }
}

/// The key the not-yet-a-workflow thread is filed under.
///
/// A workflow that does not exist has no id to key its conversation by, but the
/// thread still has to survive the operator walking away from the New row and
/// coming back. The leading control character cannot occur in an id: those come
/// from file stems, and the store never yields one containing `\u{1}`.
pub(in crate::ui::app) const NEW_THREAD: &str = "\u{1}new-workflow";

impl App {
    /// The copilot thread the rail cursor is on, creating it on first use.
    ///
    /// The New row has a thread of its own, filed under [`NEW_THREAD`] — the
    /// conversation that will produce a workflow, held before there is one to
    /// name it after.
    ///
    /// Returns `None` only when a workflow is selected and the catalogue cannot
    /// produce it.
    pub(in crate::ui::app) fn copilot_mut(&mut self) -> Option<&mut CopilotState> {
        let id = self.copilot_key()?;
        let restored = self.restored_thread(&id);
        Some(self.wf.copilots.entry(id).or_insert(restored))
    }

    /// Seed the selected thread from disk if this session has not touched it.
    ///
    /// Cheap after the first call per thread — the entry is already in the map
    /// — so the selection can move freely without re-reading anything.
    pub(in crate::ui::app) fn ensure_copilot_thread(&mut self) {
        let _ = self.copilot_mut();
    }

    /// The saved conversations for this workspace, or `None` on an app whose
    /// Medulla home was never set.
    ///
    /// Derived from the home the app resolved at startup rather than
    /// rediscovered from the process environment, so a `--home` the operator
    /// passed is honoured — and so a test fixture's conversations land in its
    /// own temporary directory instead of the developer's state.
    fn transcripts(&self) -> Option<Transcripts> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        Some(Transcripts::under(self.medulla_home.as_deref()?, &cwd))
    }

    /// A thread seeded with whatever was saved for `key`.
    ///
    /// This is the whole of "the conversation survived a restart" from the
    /// pane's side: the first time a thread is touched in a session it is read
    /// off disk rather than started empty, so opening a workflow shows what was
    /// last said about it instead of a blank pane.
    fn restored_thread(&self, key: &str) -> CopilotState {
        let mut state = CopilotState::new(key.to_string());
        if let Some(transcripts) = self.transcripts() {
            state.turns = transcripts.load(thread_of(key)).turns;
        }
        state
    }

    /// The thread the rail cursor is on, if it has one yet.
    pub(in crate::ui::app) fn copilot(&self) -> Option<&CopilotState> {
        self.wf.copilots.get(&self.copilot_key()?)
    }

    /// Which thread the rail cursor addresses.
    fn copilot_key(&self) -> Option<String> {
        if self.wf.creating {
            return Some(NEW_THREAD.to_string());
        }
        Some(self.selected_workflow()?.id.clone())
    }

    /// Whether the selected workflow has a turn in flight.
    pub(in crate::ui::app) fn copilot_busy(&self) -> bool {
        self.copilot().is_some_and(|thread| thread.busy)
    }

    /// Seed the transcript for a review started automatically after failure.
    pub fn copilot_started(&mut self, workflow: &str, instruction: &str) {
        let restored = self.restored_thread(workflow);
        self.wf
            .copilots
            .entry(workflow.to_string())
            .or_insert(restored)
            .ask(instruction);
    }

    /// Rows one `PageUp`/`PageDown` moves the copilot transcript by.
    ///
    /// A page less one row, so the line the operator was reading stays on
    /// screen and gives them their place back. Derived from the last drawn area
    /// rather than a constant: the pane's height depends on the terminal, and a
    /// fixed step pages twice on a tall one and overshoots on a short one.
    pub(in crate::ui::app) fn copilot_page(&self) -> usize {
        // The header, tab strip, footer, and status row above the tab, then the
        // pane's own borders, its hint row, and the smallest composer. An
        // estimate rather than the measured rect, because the transcript's
        // height is only known inside a draw pass — and being a row out only
        // changes how far one keypress travels, which the render then clamps.
        const CHROME: usize = 11;
        const MIN_STEP: usize = 1;
        (self.area.height as usize)
            .saturating_sub(CHROME)
            .max(MIN_STEP)
    }

    /// Send the composer's draft to the copilot.
    ///
    /// Queues rather than refusing while a turn is in flight. That reversed an
    /// earlier decision, and the reason it reversed is that the pane became a
    /// conversation: a follow-up now lands in a session that has seen the first
    /// turn's edit, which is the context that makes "and the other one too"
    /// mean anything.
    pub(in crate::ui::app) fn submit_copilot(&mut self) -> Option<Cmd> {
        let instruction = self.wf.draft.text.trim().to_string();
        if instruction.is_empty() {
            return None;
        }
        if self.copilot_busy() {
            self.wf.draft = crate::ui::composer::Draft::new();
            self.copilot_mut()?.queue(&instruction);
            self.set_status("Queued — it will go when this turn finishes");
            return None;
        }
        self.wf.draft = crate::ui::composer::Draft::new();
        self.dispatch_copilot(instruction)
    }

    /// Send `instruction` now, marking the thread busy.
    ///
    /// Shared by the composer, the queue drain, and retry, so all three record
    /// the turn the same way — a queued instruction that skipped `ask` would
    /// run without appearing in the transcript that is meant to be the record.
    fn dispatch_copilot(&mut self, instruction: String) -> Option<Cmd> {
        // The New row's turn creates rather than edits, so it is a different
        // command — but the same thread bookkeeping either way.
        let creating = self.wf.creating;
        let workflow = if creating {
            NEW_THREAD.to_string()
        } else {
            self.selected_workflow()?.id.clone()
        };
        self.wf.copilot_scroll = 0;
        self.copilot_mut()?.ask(&instruction);
        if creating {
            self.set_status("Copilot · building a new workflow…");
            return Some(Cmd::CreateWorkflow {
                thread: workflow,
                instruction,
            });
        }
        self.set_status(format!("Copilot · {workflow}"));
        Some(Cmd::CopilotTurn {
            workflow,
            instruction,
        })
    }

    /// Send the last failed instruction again.
    ///
    /// The whole of what retry is: a timeout should cost the operator the two
    /// minutes it took, not the sentence they wrote.
    pub(in crate::ui::app) fn retry_copilot(&mut self) -> Option<Cmd> {
        if self.copilot_busy() {
            self.set_status("Copilot is still busy — wait for it to finish before retrying");
            return None;
        }
        let Some(instruction) = self.copilot_mut()?.take_failed() else {
            self.set_status("Nothing to retry");
            return None;
        };
        self.dispatch_copilot(instruction)
    }

    /// Stop the turn running on this thread.
    pub(in crate::ui::app) fn abort_copilot(&mut self) -> Option<Cmd> {
        if !self.copilot_busy() {
            self.set_status("The copilot is not running");
            return None;
        }
        // Anything waiting behind it is dropped too: the operator is stopping
        // this line of work, and running the follow-up they queued against it
        // would be finishing what they just interrupted.
        self.copilot_mut()?.take_queued();
        Some(Cmd::AbortCopilot {
            thread: self.copilot_key()?,
        })
    }

    /// Ask the copilot to diagnose the selected run and fix what caused it.
    ///
    /// Refused rather than queued while a turn is already running: unlike a
    /// composer submission, a repair carries a specific `run_id` the drain path
    /// (`drain_copilot_queue`) has nowhere to hold, so queuing it would either
    /// drop that context or dispatch two turns on the same thread at once —
    /// whichever ran second would arrive out of order and its completion could
    /// clear `busy` while the other turn was still in flight.
    pub(in crate::ui::app) fn repair_selected_run(&mut self) -> Option<Cmd> {
        let Some(run) = self.selected_run().cloned() else {
            // Reported rather than silently swallowed: without this, `f` with
            // no run selected is indistinguishable on screen from a stray
            // keypress the menu absorbed on purpose.
            self.set_status("No run selected — nothing to repair");
            return None;
        };
        if run.status != medulla::workflows::RunStatus::Failed {
            self.set_status("That run did not fail — nothing to repair");
            return None;
        }
        if self.copilot_busy() {
            self.set_status("Copilot is still busy — wait for it to finish before repairing");
            return None;
        }
        let workflow = run.workflow_id.clone();
        self.wf.copilot_scroll = 0;
        let instruction = format!("Run {} failed. Work out why and fix it.", run.id);
        self.copilot_mut()?.ask(&instruction);
        self.wf.focus = super::super::types::WorkflowFocus::Copilot;
        self.set_status(format!("Copilot · diagnosing {}", run.id));
        Some(Cmd::RepairWorkflow {
            workflow,
            instruction,
            run_id: run.id,
        })
    }

    /// Record a progress line from a running copilot turn.
    ///
    /// Addressed by workflow id rather than applied to the selection: the
    /// operator may well have moved the rail on while the turn runs, and the
    /// line belongs to the thread that asked for it.
    ///
    /// Handed to [`CopilotState::progress`] rather than `status` so a frame
    /// announcing a tool call becomes a tool line — the same `⏺` the
    /// orchestrator's transcript draws — instead of dim chatter that ages out.
    pub fn copilot_status(&mut self, workflow: &str, line: String) {
        if let Some(thread) = self.wf.copilots.get_mut(workflow) {
            thread.progress(&line);
        }
    }

    /// Record a finished copilot turn: its reply, what it changed, and any
    /// workflow it brought into existence.
    ///
    /// Re-reads the catalogue and the graph when something changed, so the pane
    /// beside the transcript shows the edit the transcript just described.
    pub fn copilot_finished(
        &mut self,
        workflow: &str,
        reply: String,
        changes: Vec<String>,
        created: Option<String>,
    ) -> Option<Cmd> {
        let changed = !changes.is_empty();
        if let Some(thread) = self.wf.copilots.get_mut(workflow) {
            thread.changed(changes);
            thread.reply(reply);
        }
        // Saved before the thread can be moved or the catalogue reloaded, so a
        // crash between here and the next turn still costs nothing: what the
        // operator asked and what the agent answered are already on disk.
        self.persist_copilot(workflow);
        // `adopt_new_workflow` can move this thread — queued instruction and
        // all — from `NEW_THREAD` to the workflow's real id, so the queue must
        // be drained under whatever key the thread lives at *after* that move.
        // Draining under the original `workflow` (still `NEW_THREAD`) would
        // silently drop a follow-up an operator queued during a create turn:
        // `self.wf.copilots.get_mut(NEW_THREAD)` finds nothing once the thread
        // has relocated.
        let mut drain_key = workflow.to_string();
        if changed {
            self.reload_workflows();
            match created {
                Some(id) => {
                    if self.adopt_new_workflow(&id) {
                        drain_key = id;
                    }
                }
                None => self.set_status(format!("{workflow} updated")),
            }
        }
        if self
            .wf
            .copilots
            .get(&drain_key)
            .is_some_and(|thread| thread.busy)
        {
            return None;
        }
        // Drained after the catalogue refresh, so the queued turn is dispatched
        // against the graph this one actually left behind rather than the one
        // on screen when it was typed.
        self.drain_copilot_queue(&drain_key)
    }

    /// Send whatever was queued on `workflow`'s thread while it was busy.
    fn drain_copilot_queue(&mut self, workflow: &str) -> Option<Cmd> {
        let queued = self.wf.copilots.get_mut(workflow)?.take_queued()?;
        // Addressed by thread rather than by selection: the operator may have
        // moved the rail on, and the instruction belongs where it was typed.
        self.wf.copilots.get_mut(workflow)?.ask(&queued);
        if workflow == NEW_THREAD {
            return Some(Cmd::CreateWorkflow {
                thread: workflow.to_string(),
                instruction: queued,
            });
        }
        Some(Cmd::CopilotTurn {
            workflow: workflow.to_string(),
            instruction: queued,
        })
    }

    /// Select the workflow a create turn just made, and give it the thread that
    /// made it.
    ///
    /// The conversation moves rather than being discarded: it is the record of
    /// why this workflow looks the way it does, and the operator's next
    /// instruction is almost always a follow-up to it. The New row is left with
    /// a clean thread for the next workflow.
    ///
    /// Returns whether the thread actually moved to `id` — `false` when the
    /// catalogue lookup failed and the thread is still filed under
    /// [`NEW_THREAD`], which the caller needs to know to drain the right
    /// thread's queue afterwards.
    fn adopt_new_workflow(&mut self, id: &str) -> bool {
        let Some(index) = self
            .workflow_summaries()
            .iter()
            .position(|summary| summary.id == id)
        else {
            // The store says it is not there. Reported rather than silently
            // ignored: the agent believed it created something.
            self.set_status(format!("Created {id}, but it is not in the catalogue"));
            return false;
        };
        if let Some(mut thread) = self.wf.copilots.remove(NEW_THREAD) {
            thread.workflow_id = id.to_string();
            self.wf.copilots.insert(id.to_string(), thread);
        }
        // The saved copy moves with it. Left behind, the next session would
        // open the new workflow and find no history, while the conversation
        // that built it sat under a thread nobody opens.
        if let Some(transcripts) = self.transcripts() {
            transcripts.rename(Thread::Pending, Thread::Workflow(id));
        }
        // Clears `creating`, so the rail cursor lands on the new workflow and
        // the content pane draws its graph.
        self.select_workflow(index);
        self.set_status(format!("Created {id}"));
        true
    }

    /// Record a copilot turn that failed.
    ///
    /// Keeps the instruction that failed so `r` can send it again. Anything
    /// queued behind it is dropped: the operator's follow-up assumed the turn
    /// that just failed had happened.
    /// Write `workflow`'s thread to disk, so a restart comes back to it.
    ///
    /// Best effort and deliberately quiet: a transcript that could not be saved
    /// is history lost, not work lost, and interrupting an operator mid-turn to
    /// say so would cost more than it is worth. The turn itself already
    /// reported what it did.
    pub(in crate::ui::app) fn persist_copilot(&self, workflow: &str) {
        let (Some(thread), Some(transcripts)) =
            (self.wf.copilots.get(workflow), self.transcripts())
        else {
            return;
        };
        let _ = transcripts.save(thread_of(workflow), &thread.turns);
    }

    /// Drop a deleted workflow's saved conversation.
    ///
    /// Called when a turn removed the workflow it was scoped to. Kept, the file
    /// would be history for a graph that no longer exists — and would be
    /// resurrected wholesale by the next workflow to reuse the id.
    pub fn forget_copilot(&mut self, workflow: &str) {
        self.wf.copilots.remove(workflow);
        if let Some(transcripts) = self.transcripts() {
            transcripts.forget(thread_of(workflow));
        }
    }

    pub fn copilot_failed(&mut self, workflow: &str, instruction: String, error: String) {
        if let Some(thread) = self.wf.copilots.get_mut(workflow) {
            thread.failed_with(error.clone(), Some(instruction));
            // A follow-up waits for every overlapping turn. If another turn
            // remains, it still has a chance to establish the context the
            // operator queued against; the final failure drops it as before.
            if !thread.busy {
                thread.take_queued();
            }
        }
        // A failure is part of the record too: an operator coming back to this
        // thread should see that the last turn timed out rather than an
        // instruction that appears to have gone unanswered.
        self.persist_copilot(workflow);
        self.set_status(format!("Copilot failed: {error} · r to retry"));
    }
}

impl App {
    /// Review the selected workflow against its own history.
    ///
    /// With the cursor on a failed run the review leads with that run;
    /// otherwise it reads the whole history. Both are the same ask — "what
    /// should change" — so they are one key rather than two.
    pub(in crate::ui::app) fn evolve_selected_workflow(&mut self) -> Option<Cmd> {
        let workflow = self.selected_workflow()?.id.clone();
        if self.copilot_busy() {
            self.set_status("Copilot is still busy — wait for it to finish before reviewing");
            return None;
        }
        let run_id = self
            .selected_run()
            .filter(|run| run.status == medulla::workflows::RunStatus::Failed)
            .map(|run| run.id.clone());

        self.wf.copilot_scroll = 0;
        let instruction = match &run_id {
            Some(run_id) => format!("Review this workflow, starting from run {run_id}."),
            None => "Review this workflow against its history.".to_string(),
        };
        self.copilot_mut()?.ask(&instruction);
        self.wf.focus = super::super::types::WorkflowFocus::Copilot;
        self.set_status(format!("Reviewing {workflow}…"));
        Some(Cmd::EvolveWorkflow { workflow, run_id })
    }

    /// Apply the proposed change waiting on this workflow.
    pub(in crate::ui::app) fn accept_selected_proposal(&mut self) -> Option<Cmd> {
        let Some(proposal) = self.actionable_proposal() else {
            // Said rather than swallowed: `a` with nothing proposed is
            // otherwise indistinguishable from a keypress the menu absorbed.
            self.set_status("Nothing is proposed for this workflow");
            return None;
        };
        let proposal_id = proposal.id.clone();
        let workflow = proposal.workflow_id.clone();
        self.set_status("Applying the proposed change…");
        Some(Cmd::AcceptProposal {
            workflow,
            proposal_id,
        })
    }

    /// Decline the proposed change waiting on this workflow.
    ///
    /// The reason is recorded as a note, which is what stops the next review
    /// proposing the same thing again. Open the shared inline prompt so the
    /// keyboard shortcut still captures that required context.
    pub(in crate::ui::app) fn reject_selected_proposal(&mut self) -> Option<Cmd> {
        let Some(proposal) = self.visible_proposal() else {
            self.set_status("Nothing is proposed for this workflow");
            return None;
        };
        self.prompt = Some(super::super::types::Prompt {
            kind: super::super::types::PromptKind::RejectProposal {
                workflow: proposal.workflow_id.clone(),
                proposal_id: proposal.id.clone(),
            },
            title: "Why reject this proposal?".to_string(),
            draft: crate::ui::composer::Draft::new(),
        });
        self.set_status("Explain the rejection · Enter submit · Esc cancel");
        None
    }
}
