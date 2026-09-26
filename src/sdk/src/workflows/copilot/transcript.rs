//! Copilot conversations, kept across restarts.
//!
//! The pane's transcript used to live only in the process drawing it. That made
//! two things impossible, and both of them are things an operator expects of
//! anything shaped like a chat: closing Medulla and coming back to what was
//! said, and knowing what the last session concluded about a workflow that has
//! since misbehaved.
//!
//! What is stored is the *transcript*, not the harness session. Those are
//! different things and only one of them is ours: a harness session lives in
//! the daemon that opened it and dies with it, while the transcript is what the
//! operator and the agent said to each other, which Medulla observed in full.
//! So a thread reloaded in a new process shows its whole history immediately,
//! and the first instruction sent afterwards carries a recap
//! ([`Transcript::recap`]) into the brief so the agent picks the conversation
//! up rather than answering "now do the same to the other node" from nothing.
//!
//! Progress chatter is deliberately dropped on the way to disk. A turn's status
//! lines are the pane not being frozen; a week later they are noise around the
//! three lines that say what happened.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ui::workflows::{CopilotTurn, TurnRole};
use crate::workflows::store::{
    safe_component, workspace_state_dir, workspace_state_dir_under, write_atomic,
};
use crate::workflows::WorkflowError;

/// How many turns one thread keeps on disk.
///
/// A conversation about one workflow that has run past this is one whose early
/// turns are about a graph that no longer exists. Trimmed from the front, so
/// what survives is the recent end an operator is actually continuing.
const MAX_PERSISTED_TURNS: usize = 200;

/// How many turns a recap carries into the brief.
///
/// Small on purpose: this is context for an agent that has *lost* the session,
/// and a recap long enough to reconstruct the whole conversation would cost
/// more prompt than the graph it is about. The graph itself is in the brief
/// already, and it is authoritative about what the earlier turns actually did.
const RECAP_TURNS: usize = 12;

/// Which conversation a call is about.
///
/// A pane's thread is a workflow id — except while a create turn is running,
/// when the workflow being built has no id yet. Those two live in separate
/// directories rather than sharing one namespace with a sentinel key: a
/// workflow id comes from a filename an operator chose, so *any* sentinel
/// string is one an operator could name a workflow and silently collide with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thread<'a> {
    /// The conversation about a workflow that exists.
    Workflow(&'a str),
    /// The conversation that is building one, before it has a name.
    Pending,
}

/// One saved copilot conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    /// The thread this is: a workflow id, or empty for the pending one.
    pub thread: String,
    /// When it was last written, in milliseconds since the epoch.
    #[serde(default)]
    pub updated_at: u64,
    /// The transcript, oldest first, with progress chatter already dropped.
    #[serde(default)]
    pub turns: Vec<CopilotTurn>,
}

impl Transcript {
    /// A recap of the last few turns, as the text a brief carries.
    ///
    /// `None` when there is nothing worth recapping — a thread with no
    /// substantive turns yet, which is every thread on its first instruction.
    /// Returning `None` rather than an empty string is what keeps the brief
    /// from growing a heading with nothing under it.
    pub fn recap(&self) -> Option<String> {
        let turns: Vec<&CopilotTurn> = self
            .turns
            .iter()
            .rev()
            .take(RECAP_TURNS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if turns.is_empty() {
            return None;
        }
        let mut out = String::new();
        for turn in turns {
            let who = match turn.role {
                TurnRole::User => "operator",
                TurnRole::Agent => "you",
                TurnRole::Change => "changed",
                TurnRole::Error => "failed",
                // Tool rows are kept in the transcript because they are the
                // record of what a turn did, but a recap is about what was
                // *decided*; naming the calls again invites the agent to
                // re-derive its last turn instead of continuing.
                _ => continue,
            };
            out.push_str(&format!("- {who}: {}\n", turn.text.trim()));
        }
        (!out.is_empty()).then_some(out)
    }
}

/// The copilot transcripts saved for one workspace.
///
/// Scoped the same way run history is: two checkouts of one repository are two
/// workspaces, and a conversation about the graph in one of them is not about
/// the graph in the other.
#[derive(Debug, Clone)]
pub struct Transcripts {
    dir: PathBuf,
}

impl Transcripts {
    /// The transcripts under `dir`.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The transcripts for this environment and working directory.
    pub fn discover(env: &std::collections::HashMap<String, String>, cwd: &Path) -> Self {
        Self::new(workspace_state_dir(env, cwd).join("copilot"))
    }

    /// The transcripts under a Medulla home the caller already resolved.
    ///
    /// What the TUI uses. It settled its home once at startup — possibly from a
    /// flag rather than the environment — and a second derivation here could
    /// only ever disagree with it.
    pub fn under(home: &Path, cwd: &Path) -> Self {
        Self::new(workspace_state_dir_under(home, cwd).join("copilot"))
    }

    /// The file one thread is kept in, or `None` for a workflow whose id cannot
    /// be a filename.
    ///
    /// `None` rather than a sanitized guess: an id that fails the guard is one
    /// this store has no safe name for, and inventing one risks two workflows
    /// sharing a file.
    fn path(&self, thread: Thread<'_>) -> Option<PathBuf> {
        match thread {
            Thread::Workflow(id) => Some(
                self.dir
                    .join("workflows")
                    .join(format!("{}.json", safe_component(id).ok()?)),
            ),
            Thread::Pending => Some(self.dir.join("pending.json")),
        }
    }

    /// The name a thread records itself under.
    fn label(thread: Thread<'_>) -> String {
        match thread {
            Thread::Workflow(id) => id.to_string(),
            Thread::Pending => String::new(),
        }
    }

    /// Load `thread`'s conversation.
    ///
    /// A thread that has never been written, or whose file is unreadable or
    /// malformed, is an empty transcript rather than an error: this is history
    /// on a pane that works without it, and refusing to open the Workflows tab
    /// because one JSON file was truncated would be the wrong trade.
    pub fn load(&self, thread: Thread<'_>) -> Transcript {
        let empty = Transcript {
            thread: Self::label(thread),
            ..Default::default()
        };
        let Some(path) = self.path(thread) else {
            return empty;
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Transcript>(&raw).ok())
            .unwrap_or(empty)
    }

    /// Write `turns` as `thread`'s conversation, replacing what was there.
    ///
    /// Status lines are dropped and the result trimmed to the most recent
    /// [`MAX_PERSISTED_TURNS`]. Written whole rather than appended because the
    /// pane owns the transcript and can trim or settle a tool row in place —
    /// an append-only log would drift from what the operator is looking at.
    ///
    /// # Errors
    ///
    /// Fails when the directory cannot be created or the file cannot be
    /// written. A thread whose name is not a safe filename is *not* an error
    /// and writes nothing: the caller has nowhere to put it and no decision to
    /// make about that.
    pub fn save(&self, thread: Thread<'_>, turns: &[CopilotTurn]) -> Result<(), WorkflowError> {
        let Some(path) = self.path(thread) else {
            return Ok(());
        };
        let mut kept: Vec<CopilotTurn> = turns
            .iter()
            .filter(|turn| turn.role != TurnRole::Status)
            .cloned()
            .collect();
        if kept.len() > MAX_PERSISTED_TURNS {
            kept.drain(..kept.len() - MAX_PERSISTED_TURNS);
        }
        let record = Transcript {
            thread: Self::label(thread),
            updated_at: crate::clock::now_millis() as u64,
            turns: kept,
        };
        let body = serde_json::to_string_pretty(&record)
            .map_err(|err| WorkflowError::Malformed(err.to_string()))?;
        write_atomic(&path, body.as_bytes())
    }

    /// Move a thread's saved conversation onto another key.
    ///
    /// A create turn runs on a sentinel thread because the workflow it is
    /// building has no id yet. When one appears the pane's transcript moves
    /// onto it, and the saved copy has to move too — otherwise the next
    /// session reloads the new workflow's thread and finds nothing, while the
    /// conversation that built it sits under a sentinel nobody opens.
    ///
    /// Best effort: a thread that was never saved has nothing to move.
    pub fn rename(&self, from: Thread<'_>, to: Thread<'_>) {
        let (Some(from), Some(to)) = (self.path(from), self.path(to)) else {
            return;
        };
        // The two threads live in different directories, and the destination's
        // may not exist yet — a workflow created by the very first copilot turn
        // on a fresh machine has never been saved under its own id. Without
        // this the rename fails and the conversation that built it is stranded.
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::rename(from, to);
    }

    /// Delete a thread's saved conversation.
    ///
    /// Called when a workflow stops existing. Best effort for the same reason
    /// [`Self::rename`] is: there is nothing a caller would do differently on
    /// being told a file it wanted gone was already gone.
    pub fn forget(&self, thread: Thread<'_>) {
        if let Some(path) = self.path(thread) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
