//! Data types for the `sse` module.
#[allow(unused_imports)]
use super::*;
/// A completed SSE frame.
#[derive(Debug, Clone, PartialEq)]
pub struct SseFrame {
    /// Cursor value from an `id:` line, when present.
    pub id: Option<u64>,
    /// Concatenated `data:` payload (lines joined with `\n`).
    pub data: String,
}
/// Upper bound on a single SSE line and on one frame's accumulated `data:`
/// payload.
///
/// A peer that never sends a newline (or never terminates a frame) would
/// otherwise grow the parser's buffers without limit. Matches the control
/// socket's own frame ceiling, `control_socket::server::MAX_FRAME_BYTES`.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Signals that input exceeded [`MAX_FRAME_BYTES`] and was discarded.
///
/// The parser recovers by dropping everything up to the next frame boundary
/// (blank line); frames completed before the overflow are still returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SseOverflow;

impl std::fmt::Display for SseOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SSE frame exceeded {MAX_FRAME_BYTES} bytes; frame discarded"
        )
    }
}

impl std::error::Error for SseOverflow {}

/// Outcome of feeding the parser: `Err` means input was discarded.
///
/// Spelled out rather than using this module's [`Result`](crate::client::Result)
/// alias, which carries [`ClientError`](crate::client::ClientError).
pub type ParseResult = std::result::Result<(), SseOverflow>;

/// Incremental SSE line parser. Feed byte chunks; collect completed frames.
#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of a trailing incomplete UTF-8 sequence, carried to the next chunk.
    ///
    /// The transport splits the body at arbitrary byte offsets, so a multi-byte
    /// character can straddle two chunks; decoding each chunk on its own would
    /// turn it into replacement characters.
    pub(super) utf8_tail: Vec<u8>,
    /// Bytes of an incomplete trailing line.
    pub(super) line_buf: String,
    /// Accumulated `data:` payload for the in-progress frame.
    pub(super) data: String,
    /// Whether any `data:` line has been seen for the in-progress frame.
    pub(super) got_data: bool,
    /// `id:` value seen for the in-progress frame.
    pub(super) id: Option<u64>,
    /// Whether input is being dropped until the next frame boundary, after an
    /// oversized line or payload.
    pub(super) discarding: bool,
    /// Whether the parser is still inside the tail of a discarded oversized
    /// line whose terminating newline has not arrived yet.
    ///
    /// Set when an unterminated line is truncated mid-flight: its content was
    /// cleared, so the first empty line the parser sees only finishes that
    /// line, not the frame. Discarding continues until the frame's own blank
    /// line.
    pub(super) in_discarded_line: bool,
}
/// Seq-based de-duplication for reconnect replay.
///
/// Frames carrying a persisted `seq` are only accepted when they advance past
/// the cursor; frames without a seq (deltas) always pass.
#[derive(Debug, Default)]
pub struct SeqDedup {
    pub(super) cursor: Option<u64>,
}
/// Internal driver state for the reconnecting stream.
pub(super) struct StreamState {
    pub(super) http: reqwest::Client,
    pub(super) url: String,
    pub(super) parser: SseParser,
    pub(super) dedup: SeqDedup,
    pub(super) pending: VecDeque<Result<EventEnvelope>>,
    pub(super) body: Option<futures::stream::BoxStream<'static, reqwest::Result<Vec<u8>>>>,
    pub(super) first_connect: bool,
    /// Delay before the next reconnect attempt, doubled per failure and reset
    /// once a connection is established.
    pub(super) reconnect_delay_ms: u64,
}
