//! The State Synchronisation Protocol over one peer (protocol §4.3, §4.4, §6).
//!
//! A [`Session`] is a pure state machine: it is handed the current time and a
//! sequence source, and it returns the datagrams to put on the wire. It owns no
//! socket, no timer and no clock, which is what makes a 60-second blackout a
//! test that runs in microseconds and makes the same code usable over UDP today
//! and over a WebSocket if a network blocks UDP.
//!
//! # What a session does not do
//!
//! It does not verify the outer header's HMAC. A forwarder key belongs to one
//! node and is shared only with the backend, so an endpoint cannot check its
//! peer's tag. The endpoint's authentication is the AEAD of §4, whose AAD is the
//! whole cleartext header — which proves the datagram was addressed to *this*
//! node, by *that* sender, at *that* sequence. See [`crate::header`].
//!
//! It also does not detect replays. It does not need to: applying the same
//! Instruction twice is a no-op by construction (§4.3), so a duplicate is free
//! rather than dangerous. Replay defence for *routing* is the forwarder's job
//! (§5).

mod channel;
mod timing;
mod types;

#[cfg(test)]
mod tests;

use crate::crypto;
use crate::header::{OuterHeader, HEADER_LEN, MAX_DATAGRAM};
use crate::keys::SeqSource;
use crate::packet::{elapsed16, timestamp16, Packet, MAX_DIFF_LEN};
use crate::state::{MessageQueue, RowGrid, CHANNEL_MESSAGES, CHANNEL_SCREEN};

pub use channel::{Channel, MAX_SENT_STATES};
pub use timing::{
    Liveness, RttEstimator, DEGRADED_WINDOW, HEARTBEAT, LIVE_WINDOW, MAX_RTO, MIN_RTO,
    SEND_INTERVAL_MAX, SEND_INTERVAL_MIN,
};
pub use types::{SessionConfig, TransportError};

/// Largest message body channel 0 can carry in one datagram.
///
/// A queue diff frames each message with a 4-byte length and the diff itself
/// with a 4-byte count, so this is the datagram budget minus those eight bytes.
/// The protocol has no way to split one message across datagrams — §4.5's diff
/// is whole messages — so an oversized message is rejected at enqueue, where the
/// caller can still do something about it, rather than wedging the channel.
pub const MAX_MESSAGE_BYTES: usize = MAX_DIFF_LEN - 8;

/// Channels this implementation knows: 0 (messages) and 1 (screen).
const CHANNEL_COUNT: usize = 2;

/// How long an echoed timestamp stays usable (mosh's value).
///
/// Beyond this the sample would mostly measure how long we sat on it, so no
/// reply is sent and the peer simply takes no sample.
const TIMESTAMP_MAX_AGE: u64 = 1_000;

/// One endpoint's synchronised view of one peer.
pub struct Session {
    config: SessionConfig,
    messages: Channel<MessageQueue>,
    screen: Channel<RowGrid>,
    rtt: RttEstimator,
    /// When any datagram was last sent, for the send-interval floor.
    last_send_ms: Option<u64>,
    /// When a datagram was last received, for liveness (§6.2).
    last_heard_ms: Option<u64>,
    /// When the session was created, so liveness on a link that has never heard
    /// from its peer degrades from *somewhere* rather than reporting `Live`.
    started_ms: u64,
    /// The peer's newest `send_ts` and when it arrived, echoed exactly once.
    peer_timestamp: Option<(u16, u64)>,
    /// Per channel: something was received that the peer needs an
    /// acknowledgement for. Acks are per channel because `ack_num` is — a
    /// channel-0 Instruction says nothing about channel 1 (§4.4), so a global
    /// flag would let one channel swallow the other's acknowledgement.
    ack_pending: [bool; CHANNEL_COUNT],
    /// Alternates which channel is offered first, so neither can starve the other.
    rotate: bool,
}

impl Session {
    /// Start a session. Both endpoints begin at state 0 on every channel, so
    /// there is no handshake and nothing to re-establish after an outage.
    pub fn new(config: SessionConfig, now_ms: u64) -> Self {
        let queue = MessageQueue::new(config.queue_limits);
        let max_states = config.max_sent_states;
        Session {
            messages: Channel::new(CHANNEL_MESSAGES, queue, max_states),
            screen: Channel::new(CHANNEL_SCREEN, RowGrid::new(), max_states),
            config,
            rtt: RttEstimator::new(),
            last_send_ms: None,
            last_heard_ms: None,
            started_ms: now_ms,
            peer_timestamp: None,
            ack_pending: [false; CHANNEL_COUNT],
            rotate: false,
        }
    }

    /// This endpoint's node id.
    pub fn node_id(&self) -> crate::keys::NodeId {
        self.config.node_id
    }

    /// The peer's node id.
    pub fn peer_node_id(&self) -> crate::keys::NodeId {
        self.config.peer_node_id
    }

    /// The round-trip estimator, for `status()` and for tests.
    pub fn rtt(&self) -> &RttEstimator {
        &self.rtt
    }

    /// The current retransmission timeout in milliseconds (§6.1).
    pub fn rto(&self) -> u64 {
        self.rtt.rto()
    }

    /// Liveness derived from the last datagram received (§6.2).
    ///
    /// Advisory: the session keeps retransmitting in every state.
    pub fn liveness(&self, now_ms: u64) -> Liveness {
        let reference = self.last_heard_ms.unwrap_or(self.started_ms);
        Liveness::from_silence(now_ms.saturating_sub(reference))
    }

    /// Queue a message on channel 0 (reliable, ordered).
    ///
    /// # Errors
    ///
    /// [`TransportError::DiffTooLarge`] when the body exceeds
    /// [`MAX_MESSAGE_BYTES`], or [`TransportError::QueueOverflow`] — retryable —
    /// when the queue or the sent-state history is at its bound.
    pub fn queue_message(&mut self, body: Vec<u8>) -> Result<(), TransportError> {
        if body.len() > MAX_MESSAGE_BYTES {
            return Err(TransportError::DiffTooLarge {
                size: body.len(),
                limit: MAX_MESSAGE_BYTES,
            });
        }
        let mut queued = Some(body);
        self.messages
            .advance(|queue| queue.push(queued.take().expect("advance runs the change once")))
    }

    /// Take every message received on channel 0, in order.
    pub fn take_messages(&mut self) -> Vec<Vec<u8>> {
        let taken = self.messages.received_mut().drain();
        if !taken.is_empty() {
            // Acknowledging what we consumed lets the peer free it too.
            self.ack_pending[CHANNEL_MESSAGES as usize] = true;
        }
        taken
    }

    /// Replace the channel-1 grid (latest wins).
    ///
    /// # Errors
    ///
    /// [`TransportError::QueueOverflow`] when the sent-state history is at its
    /// bound.
    pub fn set_screen(&mut self, rows: Vec<Vec<u8>>) -> Result<(), TransportError> {
        let mut rows = Some(rows);
        self.screen.advance(|grid| {
            grid.set_rows(rows.take().expect("advance runs the change once"));
            Ok(())
        })
    }

    /// The grid received on channel 1.
    pub fn screen(&self) -> &RowGrid {
        self.screen.received()
    }

    /// The channel-0 state machine, for tests and diagnostics.
    pub fn messages_channel(&self) -> &Channel<MessageQueue> {
        &self.messages
    }

    /// The channel-1 state machine, for tests and diagnostics.
    pub fn screen_channel(&self) -> &Channel<RowGrid> {
        &self.screen
    }

    /// Whether anything is waiting for the peer to acknowledge it.
    pub fn has_pending_state(&self) -> bool {
        self.messages.is_pending() || self.screen.is_pending()
    }

    /// The datagrams to send at `now_ms`, if any.
    ///
    /// Send policy, from §6.1 (see the crate docs for where that section is
    /// underspecified and what this implementation chose):
    ///
    /// - a channel whose newest state has never been transmitted goes out as
    ///   soon as [`SEND_INTERVAL_MIN`] has elapsed since the last datagram;
    /// - a channel whose state was sent but not acknowledged is retransmitted
    ///   after [`Session::rto`];
    /// - a pending acknowledgement rides the next datagram, and is sent on its
    ///   own if nothing else is due;
    /// - and when nothing at all is pending, one heartbeat every [`HEARTBEAT`].
    ///
    /// # Errors
    ///
    /// [`TransportError::DiffTooLarge`] when a state change does not fit a
    /// datagram, or [`TransportError::Key`] when a sequence cannot be reserved.
    pub fn outgoing(
        &mut self,
        now_ms: u64,
        seq: &mut dyn SeqSource,
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        let mut out = Vec::new();
        if !self.floor_elapsed(now_ms) {
            return Ok(out);
        }

        let order = if self.rotate {
            [CHANNEL_SCREEN, CHANNEL_MESSAGES]
        } else {
            [CHANNEL_MESSAGES, CHANNEL_SCREEN]
        };
        // Which channels are due is decided before any datagram is built: the
        // send-interval floor paces the link over time, and two channels that
        // both came due at this instant should both go out now rather than one
        // of them waiting another interval.
        let due: Vec<u8> = order
            .into_iter()
            .filter(|id| self.channel_due(*id, now_ms))
            .collect();
        for id in due {
            out.push(self.build(id, now_ms, seq, false)?);
        }
        self.rotate = !self.rotate;

        if out.is_empty() {
            // A heartbeat and a bare acknowledgement are the same datagram: an
            // empty diff carrying the channel's current ack numbers. Only the
            // flag differs, and the flag is for the forwarder's benefit, not
            // ours.
            if let Some(id) = self.pending_ack_channel() {
                out.push(self.build(id, now_ms, seq, false)?);
            } else if self.heartbeat_due(now_ms) {
                out.push(self.build(CHANNEL_MESSAGES, now_ms, seq, true)?);
            }
        }
        Ok(out)
    }

    /// When [`Session::outgoing`] should next be called.
    ///
    /// A driver may call `outgoing` more often than this without harm — the
    /// policy above simply returns nothing — but not less often without
    /// delaying a retransmission.
    pub fn next_send_ms(&self, now_ms: u64) -> u64 {
        let floor = self
            .last_send_ms
            .map_or(now_ms, |at| at + SEND_INTERVAL_MIN);
        let mut due = self.last_send_ms.map_or(now_ms, |at| at + HEARTBEAT);
        if self.pending_ack_channel().is_some() {
            due = due.min(
                self.last_send_ms
                    .map_or(now_ms, |at| at + SEND_INTERVAL_MAX),
            );
        }
        for id in [CHANNEL_MESSAGES, CHANNEL_SCREEN] {
            if let Some(at) = self.channel_due_at(id) {
                due = due.min(at);
            }
        }
        due.max(floor)
    }

    /// Accept a datagram from the peer.
    ///
    /// Returns whether it advanced a held state — `false` covers a duplicate, a
    /// reordered datagram, a heartbeat and a bare acknowledgement, none of which
    /// are errors.
    ///
    /// # Errors
    ///
    /// [`TransportError::Header`], [`TransportError::Misaddressed`],
    /// [`TransportError::Crypto`], [`TransportError::Packet`] or
    /// [`TransportError::State`]. All of these mean "drop this datagram"; none
    /// of them mean the link is broken.
    pub fn handle_datagram(
        &mut self,
        datagram: &[u8],
        now_ms: u64,
    ) -> Result<bool, TransportError> {
        let header = OuterHeader::decode(datagram)?;
        if header.dst != self.config.node_id {
            return Err(TransportError::Misaddressed(format!(
                "addressed to {} not {}",
                header.dst, self.config.node_id
            )));
        }
        if header.src != self.config.peer_node_id {
            return Err(TransportError::Misaddressed(format!(
                "from {} not this link's peer {}",
                header.src, self.config.peer_node_id
            )));
        }
        let plaintext = crypto::open(
            &self.config.pair_key,
            header.seq,
            &header.signed_bytes(),
            &datagram[HEADER_LEN..],
        )?;
        let packet = Packet::decode(&plaintext)?;

        self.last_heard_ms = Some(now_ms);
        self.peer_timestamp = Some((packet.send_ts, now_ms));
        if packet.reply_ts != 0 {
            self.rtt
                .sample(u64::from(elapsed16(now_ms, packet.reply_ts)));
        }

        let instruction = packet.instruction;
        let applied = match instruction.channel {
            CHANNEL_MESSAGES => self.messages.apply(&instruction)?,
            CHANNEL_SCREEN => self.screen.apply(&instruction)?,
            // An unknown channel is a peer speaking a later dialect. Ignoring it
            // keeps the link working instead of failing the whole datagram.
            _ => false,
        };
        if (applied || instruction.new_num != instruction.old_num)
            && (instruction.channel as usize) < CHANNEL_COUNT
        {
            // Either we moved, or the peer tried to move us and needs to learn
            // where we actually are.
            self.ack_pending[instruction.channel as usize] = true;
        }
        Ok(applied)
    }

    /// The lowest-numbered channel owing the peer an acknowledgement.
    fn pending_ack_channel(&self) -> Option<u8> {
        self.ack_pending
            .iter()
            .position(|pending| *pending)
            .map(|index| index as u8)
    }

    /// Whether the send-interval floor has passed.
    fn floor_elapsed(&self, now_ms: u64) -> bool {
        self.last_send_ms
            .is_none_or(|at| now_ms >= at + SEND_INTERVAL_MIN)
    }

    /// Whether the idle heartbeat is due.
    fn heartbeat_due(&self, now_ms: u64) -> bool {
        self.last_send_ms.is_none_or(|at| now_ms >= at + HEARTBEAT)
    }

    /// When a channel next wants the wire, or `None` if it is idle.
    fn channel_due_at(&self, id: u8) -> Option<u64> {
        let (pending, unsent, last_sent) = self.channel_timing(id);
        if !pending {
            return None;
        }
        if unsent {
            return Some(self.last_send_ms.map_or(0, |at| at + SEND_INTERVAL_MIN));
        }
        Some(last_sent.map_or(0, |at| at + self.rto()))
    }

    /// Whether a channel is due to send at `now_ms`.
    fn channel_due(&self, id: u8, now_ms: u64) -> bool {
        self.channel_due_at(id).is_some_and(|at| now_ms >= at)
    }

    /// `(is_pending, has_unsent_change, last_sent_ms)` for a channel id.
    fn channel_timing(&self, id: u8) -> (bool, bool, Option<u64>) {
        match id {
            CHANNEL_SCREEN => (
                self.screen.is_pending(),
                self.screen.has_unsent_change(),
                self.screen.last_sent_ms(),
            ),
            _ => (
                self.messages.is_pending(),
                self.messages.has_unsent_change(),
                self.messages.last_sent_ms(),
            ),
        }
    }

    /// Build, seal and record one datagram for a channel.
    fn build(
        &mut self,
        id: u8,
        now_ms: u64,
        seq: &mut dyn SeqSource,
        heartbeat: bool,
    ) -> Result<Vec<u8>, TransportError> {
        let instruction = match id {
            CHANNEL_SCREEN => self.screen.instruction()?,
            _ => self.messages.instruction()?,
        };
        let packet = Packet {
            send_ts: timestamp16(now_ms),
            reply_ts: self.take_reply_ts(now_ms),
            instruction,
        };
        let plaintext = packet.encode()?;

        let full_seq = seq.next_seq()? | self.config.role.direction_bit();
        let mut header = OuterHeader::new(self.config.node_id, self.config.peer_node_id, full_seq);
        if heartbeat {
            header = header.heartbeat();
        }
        let mut datagram = Vec::with_capacity(HEADER_LEN + plaintext.len() + crypto::AEAD_OVERHEAD);
        datagram.extend_from_slice(&header.encode(&self.config.forwarder_key));
        datagram.extend_from_slice(&crypto::seal(
            &self.config.pair_key,
            full_seq,
            &header.signed_bytes(),
            &plaintext,
        ));
        debug_assert!(
            datagram.len() <= MAX_DATAGRAM,
            "datagram of {} bytes exceeds the {MAX_DATAGRAM}-byte limit",
            datagram.len()
        );

        match id {
            CHANNEL_SCREEN => self.screen.note_sent(&packet.instruction, now_ms),
            _ => self.messages.note_sent(&packet.instruction, now_ms),
        }
        self.last_send_ms = Some(now_ms);
        self.ack_pending[usize::from(id).min(CHANNEL_COUNT - 1)] = false;
        Ok(datagram)
    }

    /// The `reply_ts` to echo, adjusted for how long we held the sample.
    ///
    /// mosh's construction: the echoed timestamp is advanced by the time we sat
    /// on it, so the peer's `now − reply_ts` measures the network rather than
    /// our scheduling, and it is echoed exactly once. A sample older than
    /// [`TIMESTAMP_MAX_AGE`] is dropped — 0 means "no sample" (§4.1).
    fn take_reply_ts(&mut self, now_ms: u64) -> u16 {
        match self.peer_timestamp.take() {
            Some((ts, at)) if now_ms.saturating_sub(at) < TIMESTAMP_MAX_AGE => {
                let held = (now_ms - at) as u16;
                let reply = ts.wrapping_add(held);
                // 0 is reserved for "no sample"; nudging by a millisecond is
                // cheaper than losing the sample.
                if reply == 0 {
                    1
                } else {
                    reply
                }
            }
            _ => 0,
        }
    }
}

/// A borrowed view of the fields `LinkHandle::status()` reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionStatus {
    /// Liveness derived from the last datagram received (§6.2).
    pub liveness: Liveness,
    /// Smoothed round-trip time in milliseconds, if any sample has been taken.
    pub srtt_ms: Option<f64>,
    /// The current retransmission timeout in milliseconds.
    pub rto_ms: u64,
    /// Whether any state is waiting to be acknowledged.
    pub pending: bool,
}

impl Session {
    /// A snapshot of this session's timing and liveness.
    pub fn status(&self, now_ms: u64) -> SessionStatus {
        SessionStatus {
            liveness: self.liveness(now_ms),
            srtt_ms: self.rtt.srtt(),
            rto_ms: self.rto(),
            pending: self.has_pending_state(),
        }
    }
}
