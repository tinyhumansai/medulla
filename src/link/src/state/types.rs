//! The state-synchronisation trait and its errors.

/// Why a diff could not be applied, or a state could not accept more data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// The diff's bytes do not decode under this channel's format.
    #[error("malformed diff: {0}")]
    Malformed(String),
    /// The state is at its configured bound.
    ///
    /// Surfaced upward as a *retryable* transport error rather than a fatal one:
    /// the usual cause is a peer that has been unreachable for a long time, and
    /// the caller's existing retry path is the right place to handle it.
    #[error("state bound exceeded: {0}")]
    Overflow(String),
}

/// A state object that is synchronised by diffing rather than by streaming.
///
/// This is the core of mosh's SSP and the reason the transport tolerates loss,
/// reordering and duplication without a retransmit buffer: a datagram carries
/// "here is how to get from state *m* to state *n*", not "here are the next
/// bytes". Applying the same diff twice is a no-op because the second copy no
/// longer matches the held state number, and a peer that missed forty seconds
/// of updates receives one diff to *current*, not forty seconds of replay.
///
/// Implementations must satisfy one law, which the conformance tests check:
///
/// > `let mut s = prev.clone(); s.apply_diff(&next.diff_from(prev))` yields a
/// > state equal to `next`.
pub trait SspState: Clone {
    /// The bytes that turn `prev` into `self`.
    ///
    /// `prev` is always a state this endpoint previously held, so an
    /// implementation may assume it is an ancestor of `self`.
    fn diff_from(&self, prev: &Self) -> Vec<u8>;

    /// Apply a diff produced by [`SspState::diff_from`].
    ///
    /// # Errors
    ///
    /// [`StateError::Malformed`] when the bytes do not decode, or
    /// [`StateError::Overflow`] when applying would exceed the state's bound.
    /// On either error the state MUST be left unchanged — the caller relies on
    /// a refused Instruction being a no-op.
    fn apply_diff(&mut self, diff: &[u8]) -> Result<(), StateError>;

    /// Release anything only needed to diff from a state at or below `num`.
    ///
    /// This is `throwaway_num` of §4.3 reaching the state object: a state below
    /// the peer's throwaway can never be diffed from again, so whatever it was
    /// holding on to is freed. The default does nothing, which is right for a
    /// latest-wins state that holds no history; [`super::MessageQueue`] uses it
    /// to drop delivered messages while keeping the numbering absolute.
    fn release_through(&mut self, num: u64) {
        let _ = num;
    }
}
