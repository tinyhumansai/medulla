//! One channel's half of SSP: sent-state history, assumed receiver state, and
//! the receive side's held state (protocol §4.3, §4.4).
//!
//! Channels are independent state machines. An Instruction on channel 0 says
//! nothing about channel 1 — they keep their own numbering, their own history
//! and their own ack — so a screen that has been quiet for a minute does not
//! stall a task frame, and vice versa.

use std::collections::VecDeque;

use crate::packet::{Instruction, MAX_DIFF_LEN};
use crate::state::{SspState, StateError};

use super::types::TransportError;

/// Default cap on how many sent states one channel keeps.
///
/// History only grows while the peer is not acknowledging, and only when the
/// application changes the state, so reaching this bound means "the peer has
/// been unreachable across many state changes". That is surfaced as a retryable
/// transport error rather than allowed to grow without limit (§4.5).
pub const MAX_SENT_STATES: usize = 512;

/// The sending and receiving state for one channel.
pub struct Channel<S: SspState> {
    id: u8,
    initial: S,
    current: S,
    current_num: u64,
    /// States `assumed_receiver_num ..= current_num`, oldest first. The front is
    /// always the state we believe the peer holds, which is the only state we
    /// ever diff from.
    sent_states: VecDeque<(u64, S)>,
    assumed_receiver_num: u64,
    received: S,
    received_num: u64,
    /// Highest `current_num` that has been put on the wire at least once.
    sent_num: u64,
    /// When the newest datagram for this channel went out.
    last_sent_ms: Option<u64>,
    max_sent_states: usize,
}

impl<S: SspState> Channel<S> {
    /// A channel starting from `initial` at state 0 on both sides.
    ///
    /// Both endpoints must start from the same state 0 — that is the shared
    /// origin every later diff is chained from, and it is why no handshake is
    /// needed to start talking.
    pub fn new(id: u8, initial: S, max_sent_states: usize) -> Self {
        let mut sent_states = VecDeque::new();
        sent_states.push_back((0, initial.clone()));
        Channel {
            id,
            initial: initial.clone(),
            current: initial.clone(),
            current_num: 0,
            sent_states,
            assumed_receiver_num: 0,
            received: initial,
            received_num: 0,
            sent_num: 0,
            last_sent_ms: None,
            max_sent_states,
        }
    }

    /// Rebase this channel after the peer process changes epoch.
    ///
    /// Unconsumed outbound state is preserved and becomes state 1 relative to
    /// the shared initial state; inbound state restarts at 0.
    ///
    /// Channel 0 (MessageQueue) is not preserved: its internal base tracks absolute
    /// message numbers which cannot be adjusted when renumbering states. The peer
    /// will request the complete message sequence from state 0 after restart anyway.
    pub fn reset_peer(&mut self) {
        // Preserve pending unacknowledged states by renumbering them starting from 1.
        // This ensures that if state layer fragmentation split a message into
        // intermediate states, those states remain available instead of being
        // collapsed into a single oversized state 1.
        //
        // Channel 0 (MessageQueue) is special: its base field tracks absolute
        // message numbers that cannot simply be renumbered. The peer starts at
        // state 0 after restart and will request the full sequence, so we don't
        // preserve channel 0 states.
        let mut new_sent_states = VecDeque::new();
        new_sent_states.push_back((0, self.initial.clone()));

        let mut new_num = 1u64;
        if self.id == 0 {
            for (old_num, state) in &self.sent_states {
                if *old_num > self.assumed_receiver_num {
                    let mut rebased = state.clone();
                    rebased.release_through(self.assumed_receiver_num);
                    let rebased_num = rebased
                        .reset_origin()
                        .expect("channel zero queue reports its rebased number");
                    if rebased_num > 0
                        && new_sent_states.back().map(|(num, _)| *num) != Some(rebased_num)
                    {
                        new_sent_states.push_back((rebased_num, rebased));
                    }
                }
            }
        } else {
            for state in self.current.rebase_steps() {
                new_sent_states.push_back((new_num, state));
                self.current_num = new_num;
                new_num += 1;
            }
        }

        // Channel 0 rebases every retained intermediate queue state above. Its
        // internal absolute base belongs to the retired peer session, while the
        // sequence of prefixes is needed to keep every next diff datagram-sized.
        if self.id == 0 {
            self.current_num = self
                .current
                .reset_origin()
                .expect("channel zero queue reports its rebased number");
            if self.current_num > 0
                && new_sent_states.back().map(|(num, _)| *num) != Some(self.current_num)
            {
                new_sent_states.push_back((self.current_num, self.current.clone()));
            }
        }

        // If no pending states existed (or channel 0 deliberately collapsed
        // them), current becomes state 1.
        if new_num == 1 && self.id != 0 {
            self.current_num = 1;
            new_sent_states.push_back((1, self.current.clone()));
        }

        self.sent_states = new_sent_states;
        self.assumed_receiver_num = 0;
        self.received = self.initial.clone();
        self.received_num = 0;
        self.sent_num = 0;
        self.last_sent_ms = None;
    }

    /// The channel id carried in an Instruction.
    pub fn id(&self) -> u8 {
        self.id
    }

    /// The state this endpoint is trying to synchronise to the peer.
    pub fn current(&self) -> &S {
        &self.current
    }

    /// The state number of [`Channel::current`].
    pub fn current_num(&self) -> u64 {
        self.current_num
    }

    /// The state received from the peer.
    pub fn received(&self) -> &S {
        &self.received
    }

    /// Mutable access to the received state, for a caller draining it.
    pub fn received_mut(&mut self) -> &mut S {
        &mut self.received
    }

    /// The highest peer state number applied.
    pub fn received_num(&self) -> u64 {
        self.received_num
    }

    /// The state number the peer is believed to hold.
    pub fn assumed_receiver_num(&self) -> u64 {
        self.assumed_receiver_num
    }

    /// How many sent states are retained.
    pub fn history_len(&self) -> usize {
        self.sent_states.len()
    }

    /// Whether the peer has yet to acknowledge the current state.
    pub fn is_pending(&self) -> bool {
        self.current_num > self.assumed_receiver_num
    }

    /// Whether the current state has never been transmitted.
    pub fn has_unsent_change(&self) -> bool {
        self.current_num > self.sent_num
    }

    /// When this channel last put a datagram on the wire.
    pub fn last_sent_ms(&self) -> Option<u64> {
        self.last_sent_ms
    }

    /// Advance to a new state by mutating a copy of the current one.
    ///
    /// The previous state stays in the history so it can still be diffed from —
    /// the peer may well be holding it.
    ///
    /// # Errors
    ///
    /// Whatever `change` returns, or [`TransportError::QueueOverflow`] when the
    /// history has reached its bound (a peer that has been away too long).
    pub fn advance(
        &mut self,
        change: impl FnOnce(&mut S) -> Result<(), StateError>,
    ) -> Result<(), TransportError> {
        if self.sent_states.len() >= self.max_sent_states {
            return Err(TransportError::QueueOverflow(format!(
                "channel {} is holding {} unacknowledged states",
                self.id,
                self.sent_states.len()
            )));
        }
        let mut next = self.current.clone();
        change(&mut next)?;
        self.current = next;
        self.current_num += 1;
        self.sent_states
            .push_back((self.current_num, self.current.clone()));
        Ok(())
    }

    /// Build the Instruction to send now.
    ///
    /// The diff runs from the state the peer is believed to hold to the newest
    /// state that still fits one datagram (§3.2). When the full diff is too
    /// large, an intermediate state from the history is chosen instead — that is
    /// "fragment at the state layer": the peer converges over several datagrams
    /// without any datagram-level fragmentation.
    ///
    /// # Errors
    ///
    /// [`TransportError::DiffTooLarge`] when even a single state step does not
    /// fit — the application produced a state change larger than a datagram, and
    /// no amount of retrying will move it.
    pub fn instruction(&self) -> Result<Instruction, TransportError> {
        let (_, base) = self
            .sent_states
            .front()
            .expect("a channel always retains the assumed receiver state");
        let full = self.current.diff_from(base);
        if full.len() <= MAX_DIFF_LEN {
            return Ok(self.instruction_with(self.current_num, full));
        }
        // Walk forward through the history for the largest step that fits.
        let mut chosen: Option<(u64, Vec<u8>)> = None;
        for (num, state) in self.sent_states.iter().skip(1) {
            let diff = state.diff_from(base);
            if diff.len() > MAX_DIFF_LEN {
                break;
            }
            chosen = Some((*num, diff));
        }
        match chosen {
            Some((num, diff)) => Ok(self.instruction_with(num, diff)),
            None => Err(TransportError::DiffTooLarge {
                size: full.len(),
                limit: MAX_DIFF_LEN,
            }),
        }
    }

    /// Wrap a chosen diff in the Instruction fields shared by every send.
    fn instruction_with(&self, new_num: u64, diff: Vec<u8>) -> Instruction {
        Instruction {
            channel: self.id,
            old_num: self.assumed_receiver_num,
            new_num,
            ack_num: self.received_num,
            // We will never need the peer to diff from a state we have already
            // applied past, so its history below our received number is dead.
            throwaway_num: self.received_num,
            diff,
        }
    }

    /// Record that `instruction` was transmitted at `now_ms`.
    pub fn note_sent(&mut self, instruction: &Instruction, now_ms: u64) {
        self.sent_num = self.sent_num.max(instruction.new_num);
        self.last_sent_ms = Some(now_ms);
    }

    /// Apply an Instruction from the peer.
    ///
    /// Returns whether the held state advanced. A mismatched `old_num` is
    /// **refused** and leaves the state untouched: the sender learns the truth
    /// from the next `ack_num` and re-diffs from there. That single rule is why
    /// loss, reordering and duplication need no special handling.
    ///
    /// # Errors
    ///
    /// [`TransportError::State`] when the diff itself does not decode or would
    /// overflow the receiving state.
    pub fn apply(&mut self, instruction: &Instruction) -> Result<bool, TransportError> {
        self.acknowledge(instruction.ack_num);
        // Never release past what we believe the peer holds, whatever
        // `throwaway_num` claims: a state we might still have to diff from must
        // survive a peer that miscounts.
        self.current
            .release_through(instruction.throwaway_num.min(self.assumed_receiver_num));

        if instruction.old_num != self.received_num {
            return Ok(false);
        }
        self.received.apply_diff(&instruction.diff)?;
        let advanced = instruction.new_num > self.received_num;
        self.received_num = instruction.new_num;
        Ok(advanced)
    }

    /// Move the assumed receiver state forward and free everything below it.
    fn acknowledge(&mut self, ack_num: u64) {
        if ack_num <= self.assumed_receiver_num {
            return;
        }
        // Only advance to a state we actually sent; an ack for a number we do
        // not hold would leave us with nothing to diff from.
        if !self.sent_states.iter().any(|(num, _)| *num == ack_num) {
            return;
        }
        self.assumed_receiver_num = ack_num;
        while self
            .sent_states
            .front()
            .is_some_and(|(num, _)| *num < ack_num)
        {
            self.sent_states.pop_front();
        }
    }
}
