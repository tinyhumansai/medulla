//! Sequence allocation with a persisted reservation (protocol §3.1).
//!
//! `seq` is both the forwarder's replay counter and the AEAD nonce, so it must
//! never rewind across a restart. **Nonce reuse under a shared AEAD key is a
//! confidentiality break, not a bug to fix later**, which is why this is a
//! reservation rather than a "write the counter every datagram" scheme: writing
//! per datagram is a synchronous disk write on the send path and still loses to
//! a power cut between the write and the send.
//!
//! Instead the node writes `counter + RESERVATION_BLOCK` to disk *before*
//! handing out any sequence in that block, and only rewrites once the block is
//! consumed. A crash costs at most one skipped block of sequence numbers —
//! which nothing depends on being contiguous — and can never replay one.

use std::path::{Path, PathBuf};

use super::store::write_node_state;
use super::types::{KeyResult, NodeState};

/// How many sequences a single on-disk write reserves.
///
/// Large enough that the write is off the hot path (a heartbeat every 3 s takes
/// eight hours to consume one block), small enough that the skipped range after
/// a crash is meaningless against a 63-bit space.
pub const RESERVATION_BLOCK: u64 = 10_000;

/// The highest sequence the low 63 bits can carry; bit 63 is the direction bit.
const MAX_SEQ: u64 = (1u64 << 63) - 1;

/// A source of monotonically increasing sequence numbers for one node.
///
/// One counter per *sending node*, not per peer (protocol §3.1): a node with two
/// peers draws both their datagrams' sequences from the same source, so no
/// nonce is ever reused across destinations either.
pub trait SeqSource: Send {
    /// The next sequence, without the direction bit.
    ///
    /// # Errors
    ///
    /// Fails if the reservation cannot be persisted, or if the 63-bit counter
    /// space is exhausted — at which point the node must re-key rather than
    /// wrap, because wrapping would reuse nonces.
    fn next_seq(&mut self) -> KeyResult<u64>;
}

/// A counter with no durability, for tests and for links whose key material is
/// itself ephemeral.
#[derive(Debug)]
pub struct MemorySeq {
    next: u64,
}

impl MemorySeq {
    /// Start at `start`, or at 1 when `start` is 0 — sequences begin at 1.
    pub fn new(start: u64) -> Self {
        MemorySeq { next: start.max(1) }
    }
}

impl Default for MemorySeq {
    fn default() -> Self {
        MemorySeq::new(1)
    }
}

impl SeqSource for MemorySeq {
    fn next_seq(&mut self) -> KeyResult<u64> {
        let value = self.next;
        self.next = self.next.saturating_add(1);
        Ok(value)
    }
}

/// A counter backed by the `seq_reservation` field of `node.json`.
///
/// Holds its own copy of the node state so a rewrite never has to re-read (and
/// so can never race with another field's update in this process — the identity
/// lock excludes other processes).
#[derive(Debug)]
pub struct ReservedSeq {
    path: PathBuf,
    state: NodeState,
    next: u64,
    limit: u64,
}

impl ReservedSeq {
    /// Open the reservation recorded in `state` and immediately reserve the next
    /// block on disk.
    ///
    /// Every sequence below `state.seq_reservation` is treated as spent,
    /// whatever this process actually used before, so a crash mid-block skips
    /// forward instead of rewinding.
    pub fn open(path: &Path, state: NodeState) -> KeyResult<Self> {
        let next = state.seq_reservation.max(1);
        let mut source = ReservedSeq {
            path: path.to_path_buf(),
            state,
            next,
            limit: next,
        };
        source.reserve()?;
        Ok(source)
    }

    /// The first sequence this source will hand out next.
    pub fn peek(&self) -> u64 {
        self.next
    }

    /// Persist a new reservation covering `next .. next + RESERVATION_BLOCK`.
    fn reserve(&mut self) -> KeyResult<()> {
        let limit = self.next.saturating_add(RESERVATION_BLOCK).min(MAX_SEQ);
        self.state.seq_reservation = limit;
        write_node_state(&self.path, &self.state)?;
        self.limit = limit;
        Ok(())
    }
}

impl SeqSource for ReservedSeq {
    fn next_seq(&mut self) -> KeyResult<u64> {
        if self.next >= self.limit {
            self.reserve()?;
        }
        if self.next >= MAX_SEQ {
            return Err(std::io::Error::other(
                "link sequence space exhausted; the node must re-enroll for fresh key material",
            )
            .into());
        }
        let value = self.next;
        self.next += 1;
        Ok(value)
    }
}
