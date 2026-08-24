//! The durable transcript entry and the bounded collector that produces one.

/// Entries kept for one node's turn before the collector starts dropping.
///
/// Sized for a turn an operator would actually read back. A node that emits
/// more than this is one whose interesting part is its beginning and its reply,
/// and both survive the cap.
pub const MAX_ENTRIES: usize = 512;

/// Bytes of entry text kept for one node's turn.
///
/// Counted across the whole transcript rather than per entry, so one enormous
/// tool result cannot crowd out the two hundred lines around it. The budget is
/// deliberately smaller than the run record's own
/// [`MAX_EVIDENCE_BYTES`](crate::workflows::MAX_EVIDENCE_BYTES) equivalent:
/// every step of a run carries one of these, and a surface listing a run reads
/// all of them.
pub const MAX_TEXT_BYTES: usize = 32 * 1024;

/// The longest single entry text retained; longer ones are elided in the
/// middle, keeping the head and tail that identify what the entry was.
const MAX_ENTRY_BYTES: usize = 2 * 1024;

/// Marks text elided by the per-entry cap.
const ELISION: &str = " … ";

/// One thing a harness did, in the order it did it.
///
/// The type itself lives in the engine crate now, beside the run record that
/// stores it: a bounded, flat, stringly-typed line is what every host embedding
/// the engine needs, not something particular to Medulla. What stays here is the
/// half that *is* Medulla's — folding this host's harness event stream into
/// these entries, under the caps below.
pub use tinyflows::store::TranscriptEntry;

/// Accumulates a bounded transcript from a harness's semantic event stream.
///
/// Held by the dispatch that is running one `agent` node and fed from its
/// `on_event` callback, so it sees the same events the daemon's live status
/// frames are derived from.
#[derive(Debug, Default)]
pub struct TranscriptCollector {
    entries: Vec<TranscriptEntry>,
    /// Bytes of `text` already retained, against [`MAX_TEXT_BYTES`].
    used: usize,
    /// Events refused after a cap was hit, reported in the closing entry.
    dropped: usize,
}

impl TranscriptCollector {
    /// A collector holding nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one folded event, unless a cap has been reached.
    ///
    /// `kind` and `text` come from [`super::fold`]. An entry with empty text is
    /// dropped without counting as an overflow: it carries nothing to render,
    /// and reporting it as "dropped" would tell an operator something was lost
    /// when nothing was.
    pub fn push(&mut self, kind: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self.entries.len() >= MAX_ENTRIES || self.used >= MAX_TEXT_BYTES {
            self.dropped += 1;
            return;
        }
        let text = elide(text, MAX_ENTRY_BYTES);
        self.used += text.len();
        self.entries.push(TranscriptEntry {
            at_ms: crate::clock::now_millis(),
            kind: kind.to_string(),
            text,
        });
    }

    /// Record one folded event stamped with the harness's own timestamp.
    ///
    /// Separate from [`push`](Self::push) because only the semantic-event path
    /// has a timestamp worth preferring: it is when the harness produced the
    /// line, which on a slow stream is not when we read it.
    pub fn push_at(&mut self, at_ms: i64, kind: &str, text: &str) {
        let before = self.entries.len();
        self.push(kind, text);
        if self.entries.len() > before {
            self.entries[before].at_ms = at_ms;
        }
    }

    /// The finished transcript, with an overflow note when one is owed.
    ///
    /// The note is an entry rather than a separate field so every reader —
    /// the run view, the MCP tool, a future one — shows it without having to
    /// know it exists. A transcript that just stops looks like a harness that
    /// just stopped.
    pub fn finish(mut self) -> Vec<TranscriptEntry> {
        if self.dropped > 0 {
            let dropped = self.dropped;
            self.entries.push(TranscriptEntry {
                at_ms: crate::clock::now_millis(),
                kind: "truncated".to_string(),
                text: format!(
                    "[transcript truncated — {dropped} further event{} not recorded]",
                    if dropped == 1 { "" } else { "s" }
                ),
            });
        }
        self.entries
    }
}

/// Shorten `text` to `max` bytes by removing its middle.
///
/// The middle rather than the tail: a tool result's last lines are usually the
/// error, and a message's last sentence is usually the conclusion. Truncating
/// from the end throws away the half that says how it turned out.
fn elide(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let budget = max.saturating_sub(ELISION.len());
    let head_budget = budget * 2 / 3;
    let tail_budget = budget - head_budget;
    let head_end = floor_boundary(text, head_budget);
    let tail_start = ceil_boundary(text, text.len().saturating_sub(tail_budget));
    format!("{}{ELISION}{}", &text[..head_end], &text[tail_start..])
}

/// The largest char boundary at or below `index`.
fn floor_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The smallest char boundary at or above `index`.
fn ceil_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}
