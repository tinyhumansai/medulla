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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            Self::Tool => "⏺",
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
            Self::Tool => "magenta",
            Self::Change => "yellow",
            Self::Error => "red",
        }
    }

    /// Whether the line is secondary — progress chatter rather than substance.
    pub fn dim(self) -> bool {
        matches!(self, Self::Status | Self::Tool)
    }
}

/// One line of the copilot transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotTurn {
    /// What produced it.
    pub role: TurnRole,
    /// The text, unwrapped — the renderer knows the pane width, this does not.
    pub text: String,
}

impl CopilotTurn {
    /// A turn of `role` carrying `text`.
    pub fn new(role: TurnRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
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
    /// Whether a turn is in flight. While it is, the composer refuses new
    /// instructions rather than queueing them: the agent is editing the graph
    /// under the operator, and a second instruction against the pre-edit graph
    /// is one the operator did not mean.
    pub busy: bool,
}

/// How many status lines one turn keeps.
///
/// A harness reports progress freely and the transcript is a pane, not a log:
/// past this the oldest status lines are dropped so the reply stays on screen.
/// Only status lines are trimmed — nothing the operator typed or the agent
/// concluded is ever discarded.
const MAX_STATUS_LINES: usize = 40;

impl CopilotState {
    /// A fresh thread for `workflow_id`.
    pub fn new(workflow_id: impl Into<String>) -> Self {
        Self {
            workflow_id: workflow_id.into(),
            turns: Vec::new(),
            busy: false,
        }
    }

    /// Record the operator's instruction and mark the thread busy.
    pub fn ask(&mut self, instruction: impl Into<String>) {
        self.turns
            .push(CopilotTurn::new(TurnRole::User, instruction));
        self.busy = true;
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

    /// Record a progress frame the harness reported, as the kind of line it is.
    ///
    /// The single entry point the app crate calls for anything arriving on the
    /// status channel, so the decision of what counts as a tool call is made
    /// once ([`super::progress::classify`]) rather than at each call site.
    pub fn progress(&mut self, frame: &str) {
        match super::progress::classify(frame) {
            super::progress::Progress::Tool(text) => self.tool(text),
            super::progress::Progress::Status(text) => self.status(text),
        }
    }

    /// Record the reply and end the turn.
    pub fn reply(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !text.trim().is_empty() {
            self.turns.push(CopilotTurn::new(TurnRole::Agent, text));
        }
        self.busy = false;
    }

    /// Record what the turn changed in the graph.
    pub fn changed(&mut self, changes: impl IntoIterator<Item = String>) {
        for change in changes {
            self.turns.push(CopilotTurn::new(TurnRole::Change, change));
        }
    }

    /// Record a failure and end the turn.
    pub fn failed(&mut self, text: impl Into<String>) {
        self.turns.push(CopilotTurn::new(TurnRole::Error, text));
        self.busy = false;
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
