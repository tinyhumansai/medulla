//! The transcript model for the workflow copilot.
//!
//! The copilot is a harness session pointed at one workflow (see
//! [`crate::workflows::copilot`]), and what the operator sees of it is a
//! conversation: what they asked, what the agent said back, the progress it
//! reported on the way, and what actually changed in the graph when it finished.
//!
//! Those are different kinds of line and they are styled differently, so the
//! transcript is modelled as tagged turns rather than a string. Kept here beside
//! the other UI data so the app crate renders it without owning any of the
//! judgement about what a turn *is*.

/// Who or what produced a line of the transcript.
///
/// Serialized because a transcript outlives the process drawing it (see
/// [`crate::workflows::copilot::Transcripts`]); the wire names are the variant
/// names in `snake_case`, and renaming one silently drops every saved turn that
/// used it — so add rather than rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnRole {
    /// The operator's instruction.
    User,
    /// The copilot's reply.
    Agent,
    /// Progress the harness reported mid-turn.
    Status,
    /// A tool the copilot called, already summarised to one line.
    ///
    /// Separate from [`Self::Status`] because it is the substantive half of a
    /// turn's progress: "read the graph, then patched two nodes" is the record
    /// of what happened, while "thinking" is chatter that ages out. The
    /// orchestrator's transcript has drawn tool calls this way since it existed;
    /// this is the copilot reading the same.
    Tool,
    /// A tool call that completed successfully.
    ToolSuccess,
    /// A tool call that failed.
    ToolFailure,
    /// A change the turn made to the graph.
    Change,
    /// The turn failed.
    Error,
}

impl TurnRole {
    /// The marker that opens the line.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::User => "❯",
            Self::Agent => "⏺",
            Self::Status => "·",
            Self::Tool => "↻",
            Self::ToolSuccess => "✓",
            Self::ToolFailure => "✗",
            Self::Change => "±",
            Self::Error => "✗",
        }
    }

    /// A colour name in the vocabulary the app crate maps to a theme.
    pub fn color(self) -> &'static str {
        match self {
            Self::User => "cyan",
            Self::Agent => "green",
            Self::Status => "gray",
            Self::Tool => "yellow",
            Self::ToolSuccess => "green",
            Self::ToolFailure => "red",
            Self::Change => "yellow",
            Self::Error => "red",
        }
    }

    /// Whether the line is secondary — progress chatter rather than substance.
    pub fn dim(self) -> bool {
        matches!(self, Self::Status)
    }
}

/// One line of the copilot transcript.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CopilotTurn {
    /// What produced it.
    pub role: TurnRole,
    /// The text, unwrapped — the renderer knows the pane width, this does not.
    pub text: String,
    /// Provider tool-call identity used only to correlate settlement in place.
    ///
    /// Never restored as meaningful across processes — the provider that minted
    /// it is gone — but kept in the file so a round trip is lossless and a
    /// reloaded row still renders as the tool call it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

impl CopilotTurn {
    /// A turn of `role` carrying `text`.
    pub fn new(role: TurnRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            call_id: None,
        }
    }
}

/// The copilot conversation for one workflow.
///
/// One conversation per workflow, keyed by id: switching workflows in the rail
/// must not show the previous one's thread, and coming back to it must not have
/// lost it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopilotState {
    /// The workflow this thread is about.
    pub workflow_id: String,
    /// The transcript, oldest first.
    pub turns: Vec<CopilotTurn>,
    /// Whether a turn is in flight.
    pub busy: bool,
    /// Number of turns in flight on this thread.
    ///
    /// Usually one, but an automatic failure review can start while an
    /// operator turn is already running. Counting keeps the first completion
    /// from making the still-running second turn look idle.
    pub in_flight: usize,
    /// An instruction typed while a turn was running, to send when it finishes.
    ///
    /// Queued rather than refused, which it used to be. The old reasoning was
    /// that a second instruction written against the pre-edit graph is one the
    /// operator did not mean — but that was only true while every turn was a
    /// fresh session. Now that a pane is one conversation, the follow-up lands
    /// in a session that has seen the first turn's edit, which is exactly the
    /// context that makes it intelligible.
    ///
    /// One deep on purpose: the operator gets a "queued" line and can see it
    /// waiting. A queue that accepted five would run four of them against a
    /// graph none of their authors had looked at.
    pub queued: Option<String>,
    /// The last instruction that failed, kept so it can be sent again without
    /// being retyped.
    ///
    /// A failed turn used to clear the composer and leave nothing behind, so a
    /// timeout cost the operator their whole instruction.
    pub last_failed: Option<String>,
}

/// How many status lines one turn keeps.
///
/// A harness reports progress freely and the transcript is a pane, not a log:
/// past this the oldest status lines are dropped so the reply stays on screen.
/// Only status lines are trimmed — nothing the operator typed or the agent
/// concluded is ever discarded.
const MAX_STATUS_LINES: usize = 40;
/// Maximum reasoning text retained in one updating transcript row.
const MAX_THINKING_CHARS: usize = 800;

impl CopilotState {
    /// A fresh thread for `workflow_id`.
    pub fn new(workflow_id: impl Into<String>) -> Self {
        Self {
            workflow_id: workflow_id.into(),
            turns: Vec::new(),
            busy: false,
            in_flight: 0,
            queued: None,
            last_failed: None,
        }
    }

    /// Record the operator's instruction and mark the thread busy.
    pub fn ask(&mut self, instruction: impl Into<String>) {
        self.turns
            .push(CopilotTurn::new(TurnRole::User, instruction));
        self.in_flight = self.in_flight.saturating_add(1);
        self.busy = true;
        // A new instruction supersedes the failed one: the operator has moved
        // on, and offering to retry something they have replaced would be
        // offering to undo their own correction.
        self.last_failed = None;
    }

    /// Hold an instruction typed while a turn was running.
    ///
    /// Shown in the transcript straight away, so the operator can see it
    /// waiting rather than wondering whether it registered. Replaces anything
    /// already queued — the newer instruction is the one they meant.
    pub fn queue(&mut self, instruction: impl Into<String>) {
        let instruction = instruction.into();
        self.turns.push(CopilotTurn::new(
            TurnRole::Status,
            format!("queued: {instruction}"),
        ));
        self.queued = Some(instruction);
    }

    /// Take the queued instruction, if there is one.
    pub fn take_queued(&mut self) -> Option<String> {
        self.queued.take()
    }

    /// Take the last failed instruction, if there is one to retry.
    pub fn take_failed(&mut self) -> Option<String> {
        self.last_failed.take()
    }

    /// Record a progress line from the running turn.
    pub fn status(&mut self, text: impl Into<String>) {
        let text = text.into();
        // A harness that reports the same thing twice in a row (a poll, a
        // repeated spinner label) should not fill the pane with it.
        if let Some(last) = self.turns.last() {
            if last.role == TurnRole::Status && last.text == text {
                return;
            }
        }
        self.turns.push(CopilotTurn::new(TurnRole::Status, text));
        self.trim_status();
    }

    /// Record a tool the turn called.
    ///
    /// Kept even when the same tool is called twice in a row — unlike
    /// [`Self::status`], where a repeat is a poll. Two calls to the same tool
    /// are two things that happened, and collapsing them would under-report the
    /// work.
    pub fn tool(&mut self, text: impl Into<String>) {
        self.turns.push(CopilotTurn::new(TurnRole::Tool, text));
    }

    /// Record a tool call with the provider identity used by its result.
    fn tool_with_id(&mut self, text: String, call_id: Option<String>) {
        if let Some(id) = call_id.as_deref() {
            if let Some(turn) = self
                .turns
                .iter_mut()
                .find(|turn| turn.role == TurnRole::Tool && turn.call_id.as_deref() == Some(id))
            {
                turn.text = text;
                return;
            }
        }
        let mut turn = CopilotTurn::new(TurnRole::Tool, text);
        turn.call_id = call_id;
        self.turns.push(turn);
    }

    /// Replace the live reasoning snapshot in one bounded status row.
    fn thinking(&mut self, snapshot: &str) {
        const LABEL: &str = "thinking · ";
        let snapshot = snapshot.trim();
        if snapshot.is_empty() {
            self.status("thinking");
            return;
        }
        let keep = MAX_THINKING_CHARS
            .saturating_sub(LABEL.chars().count())
            .saturating_sub(1);
        let text = if LABEL.chars().count() + snapshot.chars().count() <= MAX_THINKING_CHARS {
            format!("{LABEL}{snapshot}")
        } else {
            let tail = snapshot
                .chars()
                .rev()
                .take(keep)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>();
            format!("{LABEL}…{tail}")
        };
        if let Some(last) = self
            .turns
            .last_mut()
            .filter(|turn| turn.role == TurnRole::Status && turn.text.starts_with(LABEL))
        {
            last.text = text;
            return;
        }
        self.status(text);
    }

    /// Record a progress frame the harness reported, as the kind of line it is.
    ///
    /// The single entry point the app crate calls for anything arriving on the
    /// status channel, so the decision of what counts as a tool call is made
    /// once ([`super::progress::classify`]) rather than at each call site.
    pub fn progress(&mut self, frame: &str) {
        match super::progress::classify(frame) {
            super::progress::Progress::Tool { call_id, text } => self.tool_with_id(text, call_id),
            super::progress::Progress::ToolResult {
                failed,
                detail,
                call_id,
            } => self.settle_tool(failed, &detail, call_id.as_deref()),
            super::progress::Progress::Thinking(fragment) => self.thinking(&fragment),
            super::progress::Progress::Status(text) => self.status(text),
        }
    }

    /// Settle a tool row only when there is exactly one possible match.
    ///
    /// The shared status channel does not carry call IDs. With overlapping
    /// calls, choosing newest or oldest can attach another call's failure to
    /// the wrong operation, so ambiguous results remain generic status lines.
    fn settle_tool(&mut self, failed: bool, detail: &str, call_id: Option<&str>) {
        let index = call_id
            .and_then(|id| {
                self.turns.iter().position(|turn| {
                    turn.role == TurnRole::Tool && turn.call_id.as_deref() == Some(id)
                })
            })
            .or_else(|| {
                if call_id.is_some() {
                    return None;
                }
                let unresolved = self
                    .turns
                    .iter()
                    .enumerate()
                    .filter_map(|(index, turn)| (turn.role == TurnRole::Tool).then_some(index))
                    .take(2)
                    .collect::<Vec<_>>();
                let [index] = unresolved.as_slice() else {
                    return None;
                };
                Some(*index)
            });
        let Some(index) = index else {
            let mut status = if failed {
                "tool failed".to_string()
            } else {
                "tool completed".to_string()
            };
            if !detail.is_empty() {
                status.push_str(" · ");
                status.push_str(detail);
            }
            self.status(status);
            return;
        };
        let turn = &mut self.turns[index];
        turn.role = if failed {
            TurnRole::ToolFailure
        } else {
            TurnRole::ToolSuccess
        };
        if !detail.is_empty() {
            turn.text.push_str(" · ");
            turn.text.push_str(detail);
        }
    }

    /// Record the reply and end the turn.
    pub fn reply(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !text.trim().is_empty() {
            self.turns.push(CopilotTurn::new(TurnRole::Agent, text));
        }
        self.finish_turn();
    }

    /// Record what the turn changed in the graph.
    pub fn changed(&mut self, changes: impl IntoIterator<Item = String>) {
        for change in changes {
            self.turns.push(CopilotTurn::new(TurnRole::Change, change));
        }
    }

    /// Record a failure and end the turn.
    ///
    /// `instruction` is kept so the operator can send it again without retyping
    /// it — a turn that times out after two minutes should not also cost them
    /// the sentence they wrote.
    pub fn failed_with(&mut self, text: impl Into<String>, instruction: Option<String>) {
        self.last_failed = instruction;
        self.failed(text);
    }

    /// Record a failure and end the turn.
    pub fn failed(&mut self, text: impl Into<String>) {
        self.turns.push(CopilotTurn::new(TurnRole::Error, text));
        self.finish_turn();
    }

    /// Settle one concurrent turn without hiding any others still running.
    fn finish_turn(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.busy = self.in_flight > 0;
    }

    /// Drop the oldest status lines once there are too many of them.
    fn trim_status(&mut self) {
        let statuses = self
            .turns
            .iter()
            .filter(|turn| turn.role == TurnRole::Status)
            .count();
        let mut excess = statuses.saturating_sub(MAX_STATUS_LINES);
        if excess == 0 {
            return;
        }
        self.turns.retain(|turn| {
            if excess > 0 && turn.role == TurnRole::Status {
                excess -= 1;
                return false;
            }
            true
        });
    }
}

#[cfg(test)]
#[path = "copilot_tests.rs"]
mod tests;
