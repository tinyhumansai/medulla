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

/// One appended transcript line and its 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailLine {
    pub line_no: i64,
    pub text: String,
}

/// The outcome of one poll: any newly-located transcript (first sighting) plus
/// the lines appended since the previous poll.
#[derive(Debug, Default)]
pub struct TailPoll {
    /// Set on the poll that first locates the transcript.
    pub located: Option<LocatedSession>,
    pub lines: Vec<TailLine>,
}

/// The transcript the tailer latched onto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedSession {
    pub path: PathBuf,
    /// The harness's own session id, read from the transcript head.
    pub harness_session_id: String,
    pub cwd: Option<String>,
}

struct Active {
    path: PathBuf,
    byte_offset: u64,
    line_no: i64,
    pending: String,
}

/// A polling tailer for one wrapped session.
pub struct SessionTailer {
    env: HashMap<String, String>,
    agent: SessionAgentKind,
    cwd: String,
    start_ms: i64,
    ignored: HashSet<PathBuf>,
    active: Option<Active>,
    /// When set, only the transcript recording this session id is accepted.
    expect_session_id: Option<String>,
    /// Start reading at the transcript's current end rather than at byte zero.
    from_end: bool,
    /// Transcripts already latched onto by another tailer in this process.
    ///
    /// Only consulted for *unpinned* discovery, which is the ambiguous case: a
    /// codex session mints its own id, so a new one has nothing to pin to and
    /// discovery falls back to "the newest transcript that appeared after we
    /// launched". Two sessions starting together both match the first rollout to
    /// appear, and both tails then settle on it — one task's answer, returned
    /// twice. A claim makes the first tailer's choice exclusive.
    ///
    /// A pinned tailer ignores claims entirely: identity already decides, and a
    /// resumed session must be able to re-latch the transcript it claimed on its
    /// previous turn.
    claims: Option<Arc<Mutex<HashSet<PathBuf>>>>,
}

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
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        dir: PathBuf,
        codex_dir: PathBuf,
        env: HashMap<String, String>,
        cwd: String,
    }

    impl Fixture {
        fn new() -> Self {
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "medulla-tail-{}-{}-{id}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let codex_dir = dir.join("codex");
            let cwd = dir.join("work");
            fs::create_dir_all(&codex_dir).unwrap();
            fs::create_dir_all(&cwd).unwrap();
            let mut env = HashMap::new();
            env.insert(
                "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
                codex_dir.to_string_lossy().into_owned(),
            );
            // Steer the claude dir somewhere empty so it never interferes.
            env.insert(
                "TINYPLACE_CLAUDE_SESSIONS_DIR".to_string(),
                dir.join("claude-empty").to_string_lossy().into_owned(),
            );
            Fixture {
                dir,
                codex_dir,
                env,
                cwd: cwd.to_string_lossy().into_owned(),
            }
        }

        fn meta_line(&self, id: &str) -> String {
            serde_json::json!({
                "type": "session_meta",
                "payload": { "session_id": id, "cwd": self.cwd }
            })
            .to_string()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn agent_message(text: &str) -> String {
        serde_json::json!({
            "type": "event_msg",
            "timestamp": "2026-07-05T00:00:00.000Z",
            "payload": { "type": "agent_message", "message": text }
        })
        .to_string()
    }

    #[test]
    fn resuming_starts_at_the_end_of_an_existing_transcript() {
        // A session being *reused* has a transcript that already exists and is
        // older than this turn — exactly what `new` is built to ignore. Without
        // `resuming` the turn never locates anything and reports that the
        // harness never started; located at byte zero it would be worse, because
        // the previous turn's completion record is still in the file and the
        // fold would settle on it and answer the wrong question.
        let fx = Fixture::new();
        let path = fx.codex_dir.join("rollout-live.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "{}", fx.meta_line("sess-live")).unwrap();
        writeln!(file, "{}", agent_message("answer to the previous turn")).unwrap();
        file.flush().unwrap();

        // `new` alone cannot see it: pre-existing, and older than the start.
        let mut fresh = SessionTailer::new(
            fx.env.clone(),
            SessionAgentKind::Codex,
            fx.cwd.clone(),
            crate::clock::now_millis(),
        )
        .expecting("sess-live");
        assert!(
            fresh.poll().located.is_none(),
            "a fresh tailer must not adopt a transcript that predates it"
        );

        let mut resumed = SessionTailer::new(
            fx.env.clone(),
            SessionAgentKind::Codex,
            fx.cwd.clone(),
            crate::clock::now_millis(),
        )
        .resuming("sess-live");

        let first = resumed.poll();
        assert!(first.located.is_some(), "the live transcript must be found");
        assert!(
            first.lines.is_empty(),
            "history is already answered; only what comes next is this turn's: {:?}",
            first.lines
        );

        writeln!(file, "{}", agent_message("answer to this turn")).unwrap();
        file.flush().unwrap();

        let next = resumed.poll();
        let texts: Vec<&str> = next.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts.len(), 1, "got {texts:?}");
        assert!(texts[0].contains("answer to this turn"), "got {texts:?}");
    }

    #[test]
    fn locates_new_file_and_streams_appended_lines() {
        let fx = Fixture::new();
        let mut tailer = SessionTailer::new(fx.env.clone(), SessionAgentKind::Codex, &fx.cwd, 0);
        // Nothing yet.
        assert!(tailer.poll().located.is_none());

        // The child creates its transcript.
        let path = fx.codex_dir.join("rollout-abc.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "{}", fx.meta_line("codex-1")).unwrap();
        writeln!(file, "{}", agent_message("first")).unwrap();
        file.flush().unwrap();

        let poll = tailer.poll();
        let located = poll.located.expect("transcript located");
        assert_eq!(located.harness_session_id, "codex-1");
        assert_eq!(located.cwd.as_deref(), Some(fx.cwd.as_str()));
        // Two complete lines (meta + message).
        assert_eq!(poll.lines.len(), 2);
        assert_eq!(poll.lines[0].line_no, 1);
        assert_eq!(poll.lines[1].line_no, 2);
        assert!(poll.lines[1].text.contains("first"));

        // Append more; only the new line comes back.
        writeln!(file, "{}", agent_message("second")).unwrap();
        file.flush().unwrap();
        let poll = tailer.poll();
        assert!(poll.located.is_none(), "already located");
        assert_eq!(poll.lines.len(), 1);
        assert_eq!(poll.lines[0].line_no, 3);
        assert!(poll.lines[0].text.contains("second"));
    }

    #[test]
    fn holds_partial_line_until_newline_arrives() {
        let fx = Fixture::new();
        let mut tailer = SessionTailer::new(fx.env.clone(), SessionAgentKind::Codex, &fx.cwd, 0);
        let path = fx.codex_dir.join("rollout-partial.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "{}", fx.meta_line("codex-2")).unwrap();
        // Write a line with no trailing newline yet.
        write!(file, "{}", agent_message("incomplete")).unwrap();
        file.flush().unwrap();

        let poll = tailer.poll();
        assert!(poll.located.is_some());
        // Only the terminated meta line surfaces; the partial is buffered.
        assert_eq!(poll.lines.len(), 1);

        // Finish the line.
        writeln!(file).unwrap();
        file.flush().unwrap();
        let poll = tailer.poll();
        assert_eq!(poll.lines.len(), 1);
        assert!(poll.lines[0].text.contains("incomplete"));
        assert_eq!(poll.lines[0].line_no, 2);
    }

    #[test]
    fn ignores_preexisting_transcripts() {
        let fx = Fixture::new();
        // A transcript that exists before the tailer starts.
        let old = fx.codex_dir.join("rollout-old.jsonl");
        let mut file = fs::File::create(&old).unwrap();
        writeln!(file, "{}", fx.meta_line("codex-old")).unwrap();
        file.flush().unwrap();

        let mut tailer = SessionTailer::new(fx.env.clone(), SessionAgentKind::Codex, &fx.cwd, 0);
        // Even after a poll, the pre-existing file is not latched.
        assert!(tailer.poll().located.is_none());
    }

    #[test]
    fn resets_on_truncation() {
        let fx = Fixture::new();
        let mut tailer = SessionTailer::new(fx.env.clone(), SessionAgentKind::Codex, &fx.cwd, 0);
        let path = fx.codex_dir.join("rollout-rot.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "{}", fx.meta_line("codex-3")).unwrap();
        writeln!(file, "{}", agent_message("aaaaaaaaaa")).unwrap();
        writeln!(file, "{}", agent_message("bbbbbbbbbb")).unwrap();
        file.flush().unwrap();
        let poll = tailer.poll();
        assert_eq!(poll.lines.len(), 3);

        // Truncate the file to strictly fewer bytes and write fresh content.
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "{}", fx.meta_line("codex-3")).unwrap();
        writeln!(file, "{}", agent_message("c")).unwrap();
        file.flush().unwrap();
        let poll = tailer.poll();
        // The tailer detects the shrink and re-reads from the top.
        assert_eq!(poll.lines.len(), 2);
        assert!(poll.lines[1].text.contains("\"c\""));
        assert_eq!(poll.lines[0].line_no, 1, "line numbering restarts");
    }
}
