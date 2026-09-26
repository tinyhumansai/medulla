//! Data model for the history-sharing reward flow: what scanning found on disk,
//! the per-agent tallies the consent screen shows, and a transcript that has been
//! read and scrubbed ready for upload.
//!
//! These are plain data holders; the behaviour that produces them lives in
//! [`scan`](super::scan) and [`redact`](super::redact).

use std::path::PathBuf;

use crate::session_history::SessionAgentKind;

/// One local session transcript eligible for upload.
#[derive(Debug, Clone)]
pub struct HistorySessionFile {
    /// The coding agent that wrote this transcript.
    pub agent: SessionAgentKind,
    /// Absolute path to the transcript file.
    pub path: PathBuf,
    /// On-disk size in bytes, used for the "about to upload ~X MB" estimate.
    pub size_bytes: u64,
    /// Last-modified epoch ms, used to prefer recent sessions when capping.
    pub mtime_ms: i64,
}

/// Per-agent totals, so the consent screen can show what each agent contributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTally {
    /// The agent these totals belong to.
    pub agent: SessionAgentKind,
    /// Number of transcripts found for this agent.
    pub session_count: usize,
    /// Combined on-disk size of those transcripts.
    pub size_bytes: u64,
}

/// Everything scanning found locally, already capped and ordered newest-first.
#[derive(Debug, Clone, Default)]
pub struct HistoryScan {
    /// The transcripts that will be uploaded, newest first.
    pub files: Vec<HistorySessionFile>,
    /// Transcripts skipped for exceeding the per-file size limit.
    pub skipped_oversize: usize,
    /// Transcripts dropped because the session cap was reached.
    pub skipped_over_cap: usize,
}

impl HistoryScan {
    /// Number of transcripts that will be uploaded.
    pub fn session_count(&self) -> usize {
        self.files.len()
    }

    /// Combined size of the transcripts that will be uploaded.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size_bytes).sum()
    }

    /// Whether there is nothing to share.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Per-agent breakdown, ordered by agent name for stable rendering.
    pub fn tallies(&self) -> Vec<AgentTally> {
        let mut tallies: Vec<AgentTally> = Vec::new();
        for file in &self.files {
            match tallies.iter_mut().find(|tally| tally.agent == file.agent) {
                Some(tally) => {
                    tally.session_count += 1;
                    tally.size_bytes += file.size_bytes;
                }
                None => tallies.push(AgentTally {
                    agent: file.agent,
                    session_count: 1,
                    size_bytes: file.size_bytes,
                }),
            }
        }
        tallies.sort_by_key(|tally| tally.agent.as_str());
        tallies
    }
}

/// A transcript read from disk with secrets scrubbed, ready to upload.
#[derive(Debug, Clone)]
pub struct RedactedSession {
    /// The agent that wrote the transcript.
    pub agent: SessionAgentKind,
    /// Where it came from, for display and error reporting.
    pub path: PathBuf,
    /// The scrubbed transcript text.
    pub content: String,
    /// How many secrets were scrubbed, surfaced so the user can see it worked.
    pub redactions: usize,
}
