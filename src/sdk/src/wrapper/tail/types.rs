//! Data types for the `tail` module.
#[allow(unused_imports)]
use super::*;
/// One appended transcript line and its 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailLine {
    /// One-based transcript line number.
    pub line_no: i64,
    /// Line contents without the trailing newline.
    pub text: String,
}
/// The outcome of one poll: any newly-located transcript (first sighting) plus
/// the lines appended since the previous poll.
#[derive(Debug, Default)]
pub struct TailPoll {
    /// Set on the poll that first locates the transcript.
    pub located: Option<LocatedSession>,
    /// Lines appended since the previous poll.
    pub lines: Vec<TailLine>,
}
/// The transcript the tailer latched onto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedSession {
    /// Transcript path selected by discovery.
    pub path: PathBuf,
    /// The harness's own session id, read from the transcript head.
    pub harness_session_id: String,
    /// Working directory recorded in the transcript.
    pub cwd: Option<String>,
}
pub(super) struct Active {
    pub(super) path: PathBuf,
    pub(super) byte_offset: u64,
    pub(super) line_no: i64,
    pub(super) pending: String,
}
/// A polling tailer for one wrapped session.
pub struct SessionTailer {
    pub(super) env: HashMap<String, String>,
    pub(super) agent: SessionAgentKind,
    pub(super) cwd: String,
    pub(super) start_ms: i64,
    pub(super) ignored: HashSet<PathBuf>,
    pub(super) active: Option<Active>,
    /// When set, only the transcript recording this session id is accepted.
    pub(super) expect_session_id: Option<String>,
    /// Start reading at the transcript's current end rather than at byte zero.
    pub(super) from_end: bool,
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
    pub(super) claims: Option<Arc<Mutex<HashSet<PathBuf>>>>,
}
