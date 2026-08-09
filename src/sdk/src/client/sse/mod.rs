//! Hand-rolled Server-Sent Events parsing and a reconnecting event stream.
//!
//! The backend emits frames of the form:
//!
//! ```text
//! id: 42
//! data: {"seq":42,"at":...,"sessionId":"...","event":{"kind":"assistant","body":"hi"}}
//!
//! : ping
//!
//! ```
//!
//! `id:` sets the replay cursor (persisted events only; deltas omit it),
//! `data:` carries the JSON [`EventEnvelope`], comment lines (`: ping`) are
//! ignored, and a blank line terminates the current frame.

use std::collections::VecDeque;

use futures::stream::{Stream, StreamExt};

use crate::client::error::{ClientError, Result};
use crate::client::types::EventEnvelope;

mod types;
pub use types::ParseResult;
pub use types::SeqDedup;
pub use types::SseFrame;
pub use types::SseOverflow;
pub use types::SseParser;
use types::StreamState;
pub use types::MAX_FRAME_BYTES;

impl SseParser {
    /// Create an empty parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every scrap of in-progress state.
    ///
    /// Called whenever the connection is torn down: the truncated frame's
    /// partial line, payload and `id` mean nothing to the next connection, and
    /// leaving them would splice the reconnect's first bytes onto a dangling
    /// line — corrupting or losing the replayed event.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feed a chunk of raw body bytes, appending any completed frames to `out`.
    ///
    /// Decodes incrementally: a trailing incomplete UTF-8 sequence is carried
    /// over to the next chunk rather than being lossily replaced, because the
    /// transport splits the body at arbitrary byte offsets.
    ///
    /// Returns [`SseOverflow`] when a line or payload exceeded
    /// [`MAX_FRAME_BYTES`] and was discarded.
    pub fn feed_bytes(&mut self, bytes: &[u8], out: &mut Vec<SseFrame>) -> ParseResult {
        let buf = if self.utf8_tail.is_empty() {
            None
        } else {
            let mut joined = std::mem::take(&mut self.utf8_tail);
            joined.extend_from_slice(bytes);
            Some(joined)
        };
        let bytes = buf.as_deref().unwrap_or(bytes);

        let mut result = Ok(());
        let mut rest = bytes;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    if self.feed(text, out).is_err() {
                        result = Err(SseOverflow);
                    }
                    return result;
                }
                Err(e) => {
                    let (valid, tail) = rest.split_at(e.valid_up_to());
                    // `valid_up_to` bounds a prefix that is known-good UTF-8.
                    let text = std::str::from_utf8(valid).unwrap_or("");
                    if self.feed(text, out).is_err() {
                        result = Err(SseOverflow);
                    }
                    match e.error_len() {
                        // Truncated at the chunk boundary — keep it for next time.
                        None => {
                            self.utf8_tail.extend_from_slice(tail);
                            return result;
                        }
                        // Genuinely invalid: emit one replacement char and skip it.
                        Some(bad) => {
                            if self.feed("\u{fffd}", out).is_err() {
                                result = Err(SseOverflow);
                            }
                            rest = &tail[bad..];
                        }
                    }
                }
            }
        }
    }

    /// Feed a chunk of decoded text, appending any completed frames to `out`.
    ///
    /// Returns [`SseOverflow`] when a line or payload exceeded
    /// [`MAX_FRAME_BYTES`]; the offending frame is dropped and parsing resumes
    /// at the next frame boundary.
    pub fn feed(&mut self, chunk: &str, out: &mut Vec<SseFrame>) -> ParseResult {
        self.line_buf.push_str(chunk);
        let mut overflowed = false;
        loop {
            let Some(nl) = self.line_buf.find('\n') else {
                // No newline in sight: a peer that never sends one must not be
                // able to grow this buffer without limit.
                if self.line_buf.len() > MAX_FRAME_BYTES {
                    self.line_buf.clear();
                    self.discard_frame();
                    // The oversized line was cut before its terminating newline
                    // arrived. We are still inside its tail, so the next empty
                    // line must not be mistaken for the frame boundary.
                    self.in_discarded_line = true;
                    overflowed = true;
                }
                break;
            };
            // A newline-terminated line can be over the cap in a single chunk —
            // the no-newline branch above never sees it. Cloning the prefix into
            // `line` would allocate the whole thing, and comment or unknown-field
            // lines are never length-checked downstream, so enforce the cap here
            // before the copy.
            if nl > MAX_FRAME_BYTES {
                self.line_buf.drain(..=nl);
                self.discard_frame();
                // This newline terminates the oversized line: if we were already
                // inside a discarded line's tail (the no-newline branch set the
                // flag), the next blank line is a frame boundary again, not
                // another line terminator.
                self.in_discarded_line = false;
                overflowed = true;
                continue;
            }
            let mut line = self.line_buf[..nl].to_string();
            // Drain the line plus the newline from the buffer.
            self.line_buf.drain(..=nl);
            if line.ends_with('\r') {
                line.pop();
            }
            if self.discarding {
                // The just-drained line ended the oversized line whose tail was
                // discarded: its terminating newline can arrive alone as an
                // empty line or attached to late tail content, so end it
                // regardless of the line's content. This newline finishes the
                // line, not the whole frame — discarding continues to the
                // frame's own blank line.
                let ended_oversized_line = self.in_discarded_line;
                self.in_discarded_line = false;
                // A blank line ends the frame being dropped; resume parsing.
                if line.is_empty() && !ended_oversized_line {
                    self.discarding = false;
                }
                continue;
            }
            if self.feed_line(&line, out).is_err() {
                overflowed = true;
            }
        }
        if overflowed {
            Err(SseOverflow)
        } else {
            Ok(())
        }
    }

    /// Abandon the in-progress frame and skip input until the next blank line.
    fn discard_frame(&mut self) {
        self.data.clear();
        self.got_data = false;
        self.id = None;
        self.discarding = true;
    }

    /// Apply one complete input line (terminating newline already removed).
    ///
    /// A blank line terminates the in-progress frame and pushes it to `out`.
    /// Returns [`SseOverflow`] when the accumulated `data:` payload would
    /// exceed [`MAX_FRAME_BYTES`]; the frame is then dropped.
    fn feed_line(&mut self, line: &str, out: &mut Vec<SseFrame>) -> ParseResult {
        if line.is_empty() {
            // Blank line terminates the frame.
            if self.got_data || self.id.is_some() {
                out.push(SseFrame {
                    id: self.id.take(),
                    data: std::mem::take(&mut self.data),
                });
            }
            self.got_data = false;
            self.id = None;
            return Ok(());
        }
        // Comment line (`: ...`, e.g. `: ping`) — ignore.
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                // A single leading space after the colon is stripped.
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (line, ""),
        };
        match field {
            "id" => {
                if let Ok(seq) = value.trim().parse::<u64>() {
                    self.id = Some(seq);
                }
            }
            "data" => {
                // Refuse to accumulate an unbounded payload: a peer that keeps
                // sending `data:` lines without ever terminating the frame
                // would otherwise exhaust memory.
                if self.data.len() + value.len() + 1 > MAX_FRAME_BYTES {
                    self.discard_frame();
                    return Err(SseOverflow);
                }
                if self.got_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.got_data = true;
            }
            // `event:`, `retry:` and unknown fields are not used here.
            _ => {}
        }
        Ok(())
    }
}

impl SeqDedup {
    /// Start from an optional last-seen seq (the reconnect `Last-Event-ID`).
    pub fn new(start: Option<u64>) -> Self {
        Self { cursor: start }
    }

    /// The current cursor, suitable for a `Last-Event-ID` reconnect header.
    pub fn cursor(&self) -> Option<u64> {
        self.cursor
    }

    /// Decide whether a frame with the given seq should be yielded, advancing
    /// the cursor when it does.
    pub fn accept(&mut self, seq: Option<u64>) -> bool {
        match seq {
            None => true,
            Some(s) => {
                if self.cursor.map(|c| s > c).unwrap_or(true) {
                    self.cursor = Some(s);
                    true
                } else {
                    false
                }
            }
        }
    }
}

/// First reconnect delay, doubled on each further attempt.
const RECONNECT_DELAY_MIN_MS: u64 = 500;
/// Ceiling on the reconnect delay, so a backend that stays down is retried at a
/// steady low rate rather than ever more slowly.
const RECONNECT_DELAY_MAX_MS: u64 = 10_000;

impl StreamState {
    /// Open (or reopen) the SSE connection using the current cursor.
    ///
    /// Reconnects back off exponentially from [`RECONNECT_DELAY_MIN_MS`] to
    /// [`RECONNECT_DELAY_MAX_MS`]; the delay resets once a connection is
    /// established, so an ordinary end-of-body reconnect is still prompt.
    async fn connect(&mut self) -> Result<()> {
        if !self.first_connect {
            tokio::time::sleep(std::time::Duration::from_millis(self.reconnect_delay_ms)).await;
            self.reconnect_delay_ms =
                (self.reconnect_delay_ms.saturating_mul(2)).min(RECONNECT_DELAY_MAX_MS);
        }
        self.first_connect = false;
        let mut req = self
            .http
            .get(&self.url)
            .headers(self.default_headers.clone())
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(cursor) = self.dedup.cursor() {
            req = req.header("Last-Event-ID", cursor.to_string());
        }
        let resp = req.send().await?.error_for_status()?;
        // Map chunks to owned bytes so the stored stream item type stays
        // nameable without depending on `bytes` directly.
        let body = resp.bytes_stream().map(|r| r.map(|b| b.to_vec()));
        self.body = Some(body.boxed());
        self.reconnect_delay_ms = RECONNECT_DELAY_MIN_MS;
        Ok(())
    }

    /// Tear the body down so the next poll reconnects, discarding the truncated
    /// frame's parser state along with it.
    fn drop_body(&mut self) {
        self.body = None;
        // The bytes after the reconnect belong to a fresh frame stream; keeping
        // the half-read line would splice them onto it.
        self.parser.reset();
    }

    /// Convert a completed frame into a deduped, decoded envelope (if any).
    fn ingest(&mut self, frame: SseFrame) {
        if !self.dedup.accept(frame.id) {
            return;
        }
        let trimmed = frame.data.trim();
        if trimmed.is_empty() {
            return;
        }
        match serde_json::from_str::<EventEnvelope>(trimmed) {
            Ok(env) => self.pending.push_back(Ok(env)),
            Err(e) => self
                .pending
                .push_back(Err(ClientError::Decode(e.to_string()))),
        }
    }

    /// Produce the next stream item, reconnecting as needed. Returns `None`
    /// only when the stream is permanently exhausted (never, in practice —
    /// it reconnects on end-of-body).
    async fn next(&mut self) -> Option<Result<EventEnvelope>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                return Some(item);
            }
            if self.body.is_none() {
                if let Err(e) = self.connect().await {
                    // Surface the connect error, then retry on the next poll.
                    return Some(Err(e));
                }
            }
            let body = self.body.as_mut().expect("body set above");
            match body.next().await {
                Some(Ok(bytes)) => {
                    let mut frames = Vec::new();
                    let overflow = self.parser.feed_bytes(&bytes, &mut frames);
                    for frame in frames {
                        self.ingest(frame);
                    }
                    if let Err(e) = overflow {
                        // The oversized frame is gone; say so rather than
                        // letting it vanish silently.
                        self.pending
                            .push_back(Err(ClientError::Decode(e.to_string())));
                    }
                }
                Some(Err(e)) => {
                    self.drop_body();
                    return Some(Err(ClientError::Transport(e)));
                }
                None => {
                    // Server closed the connection; reconnect from cursor.
                    self.drop_body();
                }
            }
        }
    }
}

/// Build a reconnecting SSE stream of [`EventEnvelope`]s.
///
/// `url` must already include auth (`?token=<jwt>`). The stream reconnects with
/// the `Last-Event-ID` header and de-duplicates replayed frames by seq. Drop
/// the returned stream to stop.
pub fn event_stream(
    http: reqwest::Client,
    default_headers: reqwest::header::HeaderMap,
    url: String,
    last_event_id: Option<u64>,
) -> impl Stream<Item = Result<EventEnvelope>> {
    let state = StreamState {
        http,
        default_headers,
        url,
        parser: SseParser::new(),
        dedup: SeqDedup::new(last_event_id),
        pending: VecDeque::new(),
        body: None,
        first_connect: true,
        reconnect_delay_ms: RECONNECT_DELAY_MIN_MS,
    };
    futures::stream::unfold(state, |mut state| async move {
        state.next().await.map(|item| (item, state))
    })
}
