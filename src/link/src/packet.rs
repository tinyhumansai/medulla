//! The encrypted payload's framing: the plaintext of §4.1 and the `Instruction`
//! of §4.3.
//!
//! ```text
//! plaintext:  0  2  send_ts    sender's clock in ms, mod 2^16
//!             2  2  reply_ts   last send_ts heard from the peer, or 0
//!             4  …  Instruction
//!
//! Instruction: 1  channel
//!              8  old_num          state this diff applies to
//!              8  new_num          state it produces
//!              8  ack_num          highest peer state we have applied
//!              8  throwaway_num    peer may discard its states below this
//!              4  diff_len
//!              …  diff
//! ```
//!
//! `send_ts`/`reply_ts` are mosh's RTT probe: the receiver echoes `send_ts` in
//! its next `reply_ts`, and the original sender computes `now − reply_ts`. A
//! `reply_ts` of 0 means "no sample", not "zero RTT", so a zero is never fed to
//! the estimator.

use crate::header::{HEADER_LEN, MAX_DATAGRAM};

/// Bytes of the plaintext that precede the `Instruction`.
pub const TIMESTAMP_LEN: usize = 4;

/// Fixed-size prefix of an `Instruction`, before `diff`.
pub const INSTRUCTION_HEADER_LEN: usize = 1 + 8 + 8 + 8 + 8 + 4;

/// Largest plaintext that still fits a [`MAX_DATAGRAM`] datagram once the outer
/// header and the AEAD tag are added (protocol §3.2).
pub const MAX_PLAINTEXT: usize = MAX_DATAGRAM - HEADER_LEN - crate::crypto::AEAD_OVERHEAD;

/// Largest `diff` a single Instruction may carry.
///
/// Senders MUST fragment at the *state* layer — send a smaller diff — never at
/// the datagram layer, because a fragmented UDP datagram is lost whole.
pub const MAX_DIFF_LEN: usize = MAX_PLAINTEXT - TIMESTAMP_LEN - INSTRUCTION_HEADER_LEN;

/// Why a payload could not be decoded, or could not be encoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PacketError {
    /// The plaintext ended before a full frame was read.
    #[error("payload truncated: expected at least {expected} bytes, got {actual}")]
    Truncated {
        /// Bytes the frame needed.
        expected: usize,
        /// Bytes actually present.
        actual: usize,
    },
    /// `diff_len` disagreed with the bytes that followed it.
    #[error("diff_len {declared} does not match the {actual} bytes that follow")]
    DiffLength {
        /// The declared length.
        declared: usize,
        /// What was actually there.
        actual: usize,
    },
    /// The diff is larger than one datagram can carry (§3.2).
    #[error("diff of {0} bytes exceeds the {MAX_DIFF_LEN}-byte budget for one datagram")]
    DiffTooLarge(usize),
}

/// One channel's state-synchronisation step (protocol §4.3).
///
/// The receiver applies `diff` **only** when `old_num` equals the number of the
/// state it currently holds; otherwise it drops the Instruction and does
/// nothing. That single rule is the whole reliability mechanism: applying the
/// same Instruction twice is a no-op, applying a stale one is refused, and the
/// sender learns the true state from the next `ack_num`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    /// Which independent state stream this belongs to (§4.4).
    pub channel: u8,
    /// The state number this diff applies to.
    pub old_num: u64,
    /// The state number it produces.
    pub new_num: u64,
    /// The highest state of the *peer's* stream the sender has applied.
    pub ack_num: u64,
    /// The peer may discard its sent states below this number.
    pub throwaway_num: u64,
    /// Channel-specific diff bytes (§4.4).
    pub diff: Vec<u8>,
}

impl Instruction {
    /// Encode into `out`.
    ///
    /// # Errors
    ///
    /// [`PacketError::DiffTooLarge`] when the diff would not fit one datagram.
    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), PacketError> {
        if self.diff.len() > MAX_DIFF_LEN {
            return Err(PacketError::DiffTooLarge(self.diff.len()));
        }
        out.push(self.channel);
        out.extend_from_slice(&self.old_num.to_be_bytes());
        out.extend_from_slice(&self.new_num.to_be_bytes());
        out.extend_from_slice(&self.ack_num.to_be_bytes());
        out.extend_from_slice(&self.throwaway_num.to_be_bytes());
        out.extend_from_slice(&(self.diff.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.diff);
        Ok(())
    }

    /// Decode from the start of `bytes`.
    ///
    /// # Errors
    ///
    /// [`PacketError::Truncated`] or [`PacketError::DiffLength`].
    pub fn decode(bytes: &[u8]) -> Result<Self, PacketError> {
        if bytes.len() < INSTRUCTION_HEADER_LEN {
            return Err(PacketError::Truncated {
                expected: INSTRUCTION_HEADER_LEN,
                actual: bytes.len(),
            });
        }
        let read_u64 = |offset: usize| {
            let mut raw = [0u8; 8];
            raw.copy_from_slice(&bytes[offset..offset + 8]);
            u64::from_be_bytes(raw)
        };
        let mut raw_len = [0u8; 4];
        raw_len.copy_from_slice(&bytes[33..37]);
        let declared = u32::from_be_bytes(raw_len) as usize;
        let available = bytes.len() - INSTRUCTION_HEADER_LEN;
        if declared != available {
            return Err(PacketError::DiffLength {
                declared,
                actual: available,
            });
        }
        Ok(Instruction {
            channel: bytes[0],
            old_num: read_u64(1),
            new_num: read_u64(9),
            ack_num: read_u64(17),
            throwaway_num: read_u64(25),
            diff: bytes[INSTRUCTION_HEADER_LEN..].to_vec(),
        })
    }
}

/// The decrypted payload of one datagram (protocol §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// The sender's clock in milliseconds, mod 2^16.
    pub send_ts: u16,
    /// The last `send_ts` received from the peer, or 0 for "no sample".
    pub reply_ts: u16,
    /// The state-synchronisation step this datagram carries.
    pub instruction: Instruction,
}

impl Packet {
    /// Encode the plaintext.
    ///
    /// # Errors
    ///
    /// [`PacketError::DiffTooLarge`] when the diff would not fit one datagram.
    pub fn encode(&self) -> Result<Vec<u8>, PacketError> {
        let mut out = Vec::with_capacity(
            TIMESTAMP_LEN + INSTRUCTION_HEADER_LEN + self.instruction.diff.len(),
        );
        out.extend_from_slice(&self.send_ts.to_be_bytes());
        out.extend_from_slice(&self.reply_ts.to_be_bytes());
        self.instruction.encode_into(&mut out)?;
        Ok(out)
    }

    /// Decode a plaintext.
    ///
    /// # Errors
    ///
    /// [`PacketError::Truncated`] when the timestamps are missing, or any
    /// [`Instruction::decode`] error.
    pub fn decode(bytes: &[u8]) -> Result<Self, PacketError> {
        if bytes.len() < TIMESTAMP_LEN {
            return Err(PacketError::Truncated {
                expected: TIMESTAMP_LEN,
                actual: bytes.len(),
            });
        }
        Ok(Packet {
            send_ts: u16::from_be_bytes([bytes[0], bytes[1]]),
            reply_ts: u16::from_be_bytes([bytes[2], bytes[3]]),
            instruction: Instruction::decode(&bytes[TIMESTAMP_LEN..])?,
        })
    }
}

/// Truncate a millisecond clock to the 16-bit field of §4.1.
pub fn timestamp16(now_ms: u64) -> u16 {
    (now_ms % 65_536) as u16
}

/// The elapsed milliseconds between an echoed `reply_ts` and now, unwrapping the
/// 16-bit field.
///
/// The field wraps every 65.5 seconds, so a sample is only meaningful below that
/// — which is far above any RTT worth measuring. A wrapped difference is
/// returned as-is; the caller discards implausible samples.
pub fn elapsed16(now_ms: u64, echoed: u16) -> u16 {
    timestamp16(now_ms).wrapping_sub(echoed)
}

#[cfg(test)]
#[path = "packet_tests.rs"]
mod tests;
