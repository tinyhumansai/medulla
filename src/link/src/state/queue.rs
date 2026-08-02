//! Channel 0: the append-only message queue (protocol §4.5).
//!
//! ```text
//! diff:  count u32
//!        repeat count times:
//!          len  u32
//!          body len bytes
//! ```
//!
//! The state is the sequence of messages appended so far and its number is the
//! count of messages, so a diff from `old_num` to `new_num` is exactly messages
//! `old_num+1 … new_num` and `apply_diff` appends. Nothing is ever dropped: a
//! peer that was away receives everything it missed, which is what task frames
//! require and what the latest-wins screen channel deliberately does not do.

use super::types::{SspState, StateError};

/// Bounds on an outbound queue.
///
/// A sender MUST bound its queue (§4.5): a peer that is unreachable for a long
/// time would otherwise grow it without limit. On overflow the link surfaces a
/// retryable transport error, rejoining the caller's existing retry path instead
/// of inventing a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueLimits {
    /// Maximum messages held in one queue state.
    pub max_messages: usize,
    /// Maximum total bytes of message bodies.
    pub max_bytes: usize,
}

impl Default for QueueLimits {
    /// Roomy enough that only a genuinely long outage reaches it, small enough
    /// that reaching it is bounded memory rather than an OOM.
    fn default() -> Self {
        QueueLimits {
            max_messages: 1024,
            max_bytes: 8 * 1024 * 1024,
        }
    }
}

/// An ordered, append-only queue of opaque messages.
///
/// `base` counts messages that have been consumed and dropped from the front, so
/// the state number ([`MessageQueue::num`]) stays absolute across pruning: both
/// endpoints number the same message the same way for the life of the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageQueue {
    messages: Vec<Vec<u8>>,
    base: u64,
    bytes: usize,
    limits: QueueLimits,
}

impl MessageQueue {
    /// An empty queue at state 0.
    pub fn new(limits: QueueLimits) -> Self {
        MessageQueue {
            messages: Vec::new(),
            base: 0,
            bytes: 0,
            limits,
        }
    }

    /// This state's number: the total count of messages ever appended.
    pub fn num(&self) -> u64 {
        self.base + self.messages.len() as u64
    }

    /// The messages still held (those above `base`).
    pub fn pending(&self) -> &[Vec<u8>] {
        &self.messages
    }

    /// Append a message.
    ///
    /// # Errors
    ///
    /// [`StateError::Overflow`] when the queue is at its bound. The message is
    /// not appended, so the caller may retry it once the peer catches up.
    pub fn push(&mut self, message: Vec<u8>) -> Result<(), StateError> {
        self.check_capacity(1, message.len())?;
        self.bytes += message.len();
        self.messages.push(message);
        Ok(())
    }

    /// Drop everything up to and including message `num`, keeping the numbering.
    ///
    /// The sender calls this once the peer has acknowledged `num` (those
    /// messages can never be diffed from again), and the receiver calls it
    /// through [`MessageQueue::drain`] as it hands messages upward.
    pub fn prune_through(&mut self, num: u64) {
        if num <= self.base {
            return;
        }
        let drop = ((num - self.base) as usize).min(self.messages.len());
        for message in self.messages.drain(..drop) {
            self.bytes -= message.len();
        }
        self.base += drop as u64;
    }

    /// Take every held message, leaving the state number untouched.
    ///
    /// This is how a receiver hands messages to the application: the queue keeps
    /// counting, so the sender's `old_num` checks keep matching.
    pub fn drain(&mut self) -> Vec<Vec<u8>> {
        self.base += self.messages.len() as u64;
        self.bytes = 0;
        std::mem::take(&mut self.messages)
    }

    /// Whether appending `count` messages of `bytes` total would exceed the bound.
    fn check_capacity(&self, count: usize, bytes: usize) -> Result<(), StateError> {
        if self.messages.len() + count > self.limits.max_messages {
            return Err(StateError::Overflow(format!(
                "outbound queue holds {} messages, limit {}",
                self.messages.len(),
                self.limits.max_messages
            )));
        }
        if self.bytes + bytes > self.limits.max_bytes {
            return Err(StateError::Overflow(format!(
                "outbound queue holds {} bytes, limit {}",
                self.bytes, self.limits.max_bytes
            )));
        }
        Ok(())
    }
}

impl SspState for MessageQueue {
    /// Messages `prev.num()+1 … self.num()`, in order.
    ///
    /// When `prev` is ahead of what this state still holds (its messages were
    /// pruned) the diff is empty rather than wrong: the receiver's `old_num`
    /// check then refuses it and the next `ack_num` re-synchronises both sides.
    fn diff_from(&self, prev: &Self) -> Vec<u8> {
        let start = prev.num().max(self.base);
        let skip = (start - self.base) as usize;
        let sending: &[Vec<u8>] = self.messages.get(skip..).unwrap_or(&[]);
        let mut out = Vec::with_capacity(4 + sending.iter().map(|m| m.len() + 4).sum::<usize>());
        out.extend_from_slice(&(sending.len() as u32).to_be_bytes());
        for message in sending {
            out.extend_from_slice(&(message.len() as u32).to_be_bytes());
            out.extend_from_slice(message);
        }
        out
    }

    /// Drop the messages the peer has acknowledged; the numbering is unaffected.
    fn release_through(&mut self, num: u64) {
        self.prune_through(num);
    }

    fn apply_diff(&mut self, diff: &[u8]) -> Result<(), StateError> {
        let decoded = decode_messages(diff)?;
        let bytes: usize = decoded.iter().map(Vec::len).sum();
        self.check_capacity(decoded.len(), bytes)?;
        self.bytes += bytes;
        self.messages.extend(decoded);
        Ok(())
    }
}

/// Parse the §4.5 diff body. Rejects a truncated or over-declared frame rather
/// than appending a partial message.
fn decode_messages(diff: &[u8]) -> Result<Vec<Vec<u8>>, StateError> {
    let read_u32 = |bytes: &[u8], at: usize| -> Result<usize, StateError> {
        if bytes.len() < at + 4 {
            return Err(StateError::Malformed(format!(
                "queue diff truncated at offset {at}"
            )));
        }
        let mut raw = [0u8; 4];
        raw.copy_from_slice(&bytes[at..at + 4]);
        Ok(u32::from_be_bytes(raw) as usize)
    };

    let count = read_u32(diff, 0)?;
    let mut offset = 4;
    let mut out = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let len = read_u32(diff, offset)?;
        offset += 4;
        if diff.len() < offset + len {
            return Err(StateError::Malformed(format!(
                "queue diff declares a {len}-byte message with only {} bytes left",
                diff.len() - offset
            )));
        }
        out.push(diff[offset..offset + len].to_vec());
        offset += len;
    }
    if offset != diff.len() {
        return Err(StateError::Malformed(format!(
            "queue diff has {} trailing bytes",
            diff.len() - offset
        )));
    }
    Ok(out)
}
