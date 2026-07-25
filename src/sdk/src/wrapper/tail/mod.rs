//! Session-log discovery and tailing for the wrapper.
//!
//! After the child harness spawns it writes a JSONL transcript to its own
//! sessions directory (`~/.claude/projects/**`, `~/.codex/sessions/rollout-*`).
//! The tailer latches onto the newest transcript the child created — ignoring any
//! that existed before launch — then streams appended lines from a byte offset,
//! resetting on truncation/rotation. Discovery reuses
//! [`crate::session_history`]; line normalization reuses
//! [`crate::daemon::mappers`]. (opencode is out of scope here — its wrapper uses
//! an SSE bridge, a documented scope cut.)

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::session_history::{discover_session_file, preexisting_session_files, SessionAgentKind};

/// Grace applied to the discovery mtime floor: a transcript touched just before
/// launch still counts as this run's, matching the TS wrapper's `start - 2000`.
const DISCOVER_MTIME_GRACE_MS: i64 = 2_000;

impl SessionTailer {
    /// Build a tailer for `agent` anchored at `cwd`. `start_ms` is the launch
    /// instant; pre-existing transcripts are snapshotted now and ignored.
    pub fn new(
        env: HashMap<String, String>,
        agent: SessionAgentKind,
        cwd: impl Into<String>,
        start_ms: i64,
    ) -> Self {
        let ignored = preexisting_session_files(&env, agent);
        SessionTailer {
            env,
            agent,
            cwd: cwd.into(),
            start_ms,
            ignored,
            active: None,
            expect_session_id: None,
            from_end: false,
            claims: None,
        }
    }

    /// Share `claims` with every other tailer that may discover the same
    /// directory, so an unpinned discovery cannot take a transcript another
    /// tailer already holds.
    pub fn with_claims(mut self, claims: Arc<Mutex<HashSet<PathBuf>>>) -> Self {
        self.claims = Some(claims);
        self
    }

    /// Pin this tailer to one session id.
    ///
    /// Without a pin the tailer takes the newest transcript in `cwd`, which is
    /// correct for one session per directory and wrong for any other number:
    /// two concurrent sessions in one repo make the choice flip-flop, and the
    /// consequence is a reply carrying another session's answer. With a pin,
    /// identity beats recency and a mismatch simply stays unlocated.
    pub fn expecting(mut self, session_id: impl Into<String>) -> Self {
        self.expect_session_id = Some(session_id.into());
        self
    }

    /// Tail a session that is **already running**, from where its transcript
    /// currently ends.
    ///
    /// [`new`](Self::new) is built for a session about to start: it snapshots
    /// the transcripts that already exist and ignores them, and it discounts any
    /// file older than the launch instant, so the one new file that appears is
    /// unambiguously this session's. Every one of those rules is exactly wrong
    /// for a session being reused — its transcript is by definition pre-existing
    /// and older than this turn — and the symptom is a turn that never locates
    /// anything and reports that the harness never started.
    ///
    /// So identity replaces recency: the pinned id decides, and the read starts
    /// at the file's current end. Starting at byte zero would be worse than not
    /// locating it at all, because the completion record of the *previous* turn
    /// is still in the file — the fold would settle on it immediately and hand
    /// the peer the answer to the question it asked last time.
    pub fn resuming(mut self, session_id: impl Into<String>) -> Self {
        self.expect_session_id = Some(session_id.into());
        self.ignored.clear();
        self.start_ms = 0;
        self.from_end = true;
        self
    }

    /// The session id this tailer is pinned to, if any.
    pub fn expected_session_id(&self) -> Option<&str> {
        self.expect_session_id.as_deref()
    }

    /// Whether the transcript has been located yet.
    pub fn is_located(&self) -> bool {
        self.active.is_some()
    }

    /// Poll once: locate the transcript if needed, then read appended lines.
    pub fn poll(&mut self) -> TailPoll {
        let mut out = TailPoll::default();
        if self.active.is_none() {
            // Claims only bind unpinned discovery; see the field's docs.
            let skip = match (&self.claims, self.expect_session_id.is_some()) {
                (Some(claims), false) => {
                    let mut skip = self.ignored.clone();
                    skip.extend(claims.lock().expect("claim lock").iter().cloned());
                    std::borrow::Cow::Owned(skip)
                }
                _ => std::borrow::Cow::Borrowed(&self.ignored),
            };
            match discover_session_file(
                &self.env,
                self.agent,
                &self.cwd,
                self.start_ms - DISCOVER_MTIME_GRACE_MS,
                &skip,
                self.expect_session_id.as_deref(),
            ) {
                Some(found) => {
                    if let Some(claims) = &self.claims {
                        claims
                            .lock()
                            .expect("claim lock")
                            .insert(found.path.clone());
                    }
                    out.located = Some(LocatedSession {
                        path: found.path.clone(),
                        harness_session_id: found.id,
                        cwd: found.cwd,
                    });
                    // A resumed tail opens at the end: everything before this
                    // point belongs to turns that are already answered.
                    let byte_offset = if self.from_end {
                        std::fs::metadata(&found.path).map(|m| m.len()).unwrap_or(0)
                    } else {
                        0
                    };
                    self.active = Some(Active {
                        path: found.path,
                        byte_offset,
                        line_no: 0,
                        pending: String::new(),
                    });
                }
                None => return out,
            }
        }
        out.lines = self.read_appended();
        out
    }

    /// Drain the transcript one final time (final poll on teardown).
    pub fn drain(&mut self) -> Vec<TailLine> {
        if self.active.is_none() {
            let poll = self.poll();
            return poll.lines;
        }
        self.read_appended()
    }

    fn read_appended(&mut self) -> Vec<TailLine> {
        let active = match self.active.as_mut() {
            Some(active) => active,
            None => return Vec::new(),
        };
        let mut file = match std::fs::File::open(&active.path) {
            Ok(file) => file,
            Err(_) => return Vec::new(),
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        // Truncation/rotation: the file shrank below where we last read. Restart.
        if len < active.byte_offset {
            active.byte_offset = 0;
            active.line_no = 0;
            active.pending.clear();
        }
        if file.seek(SeekFrom::Start(active.byte_offset)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        active.byte_offset += buf.len() as u64;
        let text = String::from_utf8_lossy(&buf);
        let mut combined = std::mem::take(&mut active.pending);
        combined.push_str(&text);

        let mut out = Vec::new();
        // Everything up to the final newline is complete; the tail (after the last
        // newline) is an unterminated remainder held for the next poll.
        let last_newline = combined.rfind('\n');
        let (complete, remainder) = match last_newline {
            Some(index) => (&combined[..index], &combined[index + 1..]),
            None => ("", combined.as_str()),
        };
        for raw in complete.split('\n') {
            let line = raw.trim_end_matches('\r');
            active.line_no += 1;
            if line.is_empty() {
                continue;
            }
            out.push(TailLine {
                line_no: active.line_no,
                text: line.to_string(),
            });
        }
        active.pending = remainder.to_string();
        out
    }
}

#[cfg(test)]
mod tests;

mod types;
use types::Active;
pub use types::LocatedSession;
pub use types::SessionTailer;
pub use types::TailLine;
pub use types::TailPoll;
