//! Retransmission timing and liveness (protocol §6).
//!
//! Everything here is driven by an explicit `now_ms` supplied by the caller
//! rather than by a wall clock, so the whole transport is a pure state machine:
//! the conformance tests drive a 60-second blackout in microseconds and get the
//! same arithmetic production does.

/// Floor on the interval between two datagrams (protocol §6.1).
pub const SEND_INTERVAL_MIN: u64 = 20;

/// Ceiling on how long a pending state change waits to be coalesced (§6.1).
pub const SEND_INTERVAL_MAX: u64 = 250;

/// Lower clamp on the retransmission timeout (§6.1).
pub const MIN_RTO: u64 = 50;

/// Upper clamp on the retransmission timeout (§6.1).
pub const MAX_RTO: u64 = 1000;

/// Idle heartbeat interval (§6.1).
///
/// Not optional: a NAT mapping for an idle UDP flow typically expires in about
/// 30 seconds, and both that mapping and the forwarder's binding depend on
/// traffic. 3 seconds is mosh's interval and there is no reason to differ.
pub const HEARTBEAT: u64 = 3_000;

/// Heard within this long: [`Liveness::Live`] (§6.2, `3 × HEARTBEAT`).
pub const LIVE_WINDOW: u64 = 3 * HEARTBEAT;

/// Heard within this long: [`Liveness::Degraded`]; beyond it, `Offline` (§6.2).
pub const DEGRADED_WINDOW: u64 = 60_000;

/// How reachable the peer looks, derived from the last datagram received (§6.2).
///
/// **Advisory.** SSP keeps retransmitting through all three states: an `Offline`
/// peer is not a failed one, and recovery needs no reconnect, no handshake and
/// no re-enrollment. Nothing above the link may treat `Offline` as terminal on
/// its own — its purpose is to *pause* the timeouts above the link (§6.3), not
/// to fire them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Liveness {
    /// Heard within 9 seconds.
    Live,
    /// Heard between 9 and 60 seconds ago.
    Degraded,
    /// Not heard for over 60 seconds.
    Offline,
}

impl Liveness {
    /// Derive liveness from how long ago the peer was last heard.
    pub fn from_silence(silence_ms: u64) -> Self {
        if silence_ms <= LIVE_WINDOW {
            Liveness::Live
        } else if silence_ms <= DEGRADED_WINDOW {
            Liveness::Degraded
        } else {
            Liveness::Offline
        }
    }
}

/// Smoothed round-trip estimate feeding `RTO = SRTT + 4·RTTVAR` (§6.1).
///
/// The update rule is mosh's, which is RFC 6298's: the first sample seeds
/// `SRTT = r`, `RTTVAR = r/2`; later samples decay both. Until there is a
/// sample at all — a fresh link, or one whose peer has never echoed a
/// timestamp — [`RttEstimator::rto`] returns [`MAX_RTO`], which is the
/// conservative choice: retransmitting too eagerly at an unknown RTT is how a
/// congested link is turned into a broken one.
#[derive(Debug, Clone, Copy, Default)]
pub struct RttEstimator {
    srtt: Option<f64>,
    rttvar: f64,
}

impl RttEstimator {
    /// A fresh estimator with no samples.
    pub fn new() -> Self {
        RttEstimator::default()
    }

    /// Fold in one round-trip measurement, in milliseconds.
    ///
    /// A sample of 0 ms is legitimate on a loopback link and is kept; the
    /// "no sample" case is signalled by `reply_ts == 0` upstream and never
    /// reaches here.
    pub fn sample(&mut self, rtt_ms: u64) {
        let r = rtt_ms as f64;
        match self.srtt {
            None => {
                self.srtt = Some(r);
                self.rttvar = r / 2.0;
            }
            Some(srtt) => {
                self.rttvar = 0.75 * self.rttvar + 0.25 * (srtt - r).abs();
                self.srtt = Some(0.875 * srtt + 0.125 * r);
            }
        }
    }

    /// The smoothed RTT, if any sample has been taken.
    pub fn srtt(&self) -> Option<f64> {
        self.srtt
    }

    /// The RTT variation estimate.
    pub fn rttvar(&self) -> f64 {
        self.rttvar
    }

    /// `SRTT + 4·RTTVAR`, clamped to `[MIN_RTO, MAX_RTO]`.
    pub fn rto(&self) -> u64 {
        match self.srtt {
            None => MAX_RTO,
            Some(srtt) => {
                let raw = srtt + 4.0 * self.rttvar;
                (raw.round().max(0.0) as u64).clamp(MIN_RTO, MAX_RTO)
            }
        }
    }
}
