//! A scriptable fake network for driving two [`Session`]s against each other.
//!
//! Every conformance scenario in the protocol's §9 transport list is a network
//! behaviour — loss, reordering, duplication, delay, a total blackout, a peer
//! that goes away for a minute. Running those against a real socket would mean
//! sleeping on a wall clock, which is slow and flaky in exactly the cases that
//! matter. A [`Session`] takes its time as a parameter, so instead the whole
//! link is stepped through simulated milliseconds: a 60-second blackout is a
//! loop, not a wait, and the same seed always produces the same run.

#![allow(dead_code)]

use medulla_link::header::OuterHeader;
use medulla_link::keys::{ForwarderKey, MemorySeq, NodeId, PairKey, Role};
use medulla_link::packet::Packet;
use medulla_link::state::CHANNEL_SCREEN;
use medulla_link::{Session, SessionConfig};

/// Which end of the link a datagram is travelling to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The orchestrator endpoint.
    Orchestrator,
    /// The host endpoint.
    Host,
}

impl End {
    /// The other end.
    pub fn other(self) -> End {
        match self {
            End::Orchestrator => End::Host,
            End::Host => End::Orchestrator,
        }
    }
}

/// What the network does to the datagrams crossing it.
#[derive(Debug, Clone)]
pub struct Script {
    /// Percentage of datagrams dropped outright.
    pub loss_percent: u32,
    /// Percentage of datagrams delivered twice.
    pub duplicate_percent: u32,
    /// Percentage of datagrams held back so a later one overtakes them.
    pub reorder_percent: u32,
    /// One-way delay in milliseconds.
    pub delay_ms: u64,
    /// Extra delay applied to a reordered datagram.
    pub reorder_delay_ms: u64,
    /// Windows `[from, to)` during which nothing crosses. `None` for the
    /// direction means both.
    pub blackouts: Vec<(u64, u64, Option<End>)>,
    /// Seed for the deterministic generator.
    pub seed: u64,
}

impl Default for Script {
    /// A perfect network with a 20 ms one-way delay.
    fn default() -> Self {
        Script {
            loss_percent: 0,
            duplicate_percent: 0,
            reorder_percent: 0,
            delay_ms: 20,
            reorder_delay_ms: 120,
            blackouts: Vec::new(),
            seed: 0x5EED,
        }
    }
}

impl Script {
    /// The §9 "loss, reordering and duplication" scenario.
    pub fn hostile() -> Self {
        Script {
            loss_percent: 30,
            duplicate_percent: 15,
            reorder_percent: 20,
            ..Script::default()
        }
    }

    /// Add a total blackout covering `[from, from + length)`.
    pub fn with_blackout(mut self, from: u64, length: u64) -> Self {
        self.blackouts.push((from, from + length, None));
        self
    }
}

/// A datagram in flight.
struct InFlight {
    deliver_at: u64,
    to: End,
    bytes: Vec<u8>,
}

/// Counters describing what the network did, so a test can assert the scenario
/// actually happened rather than trusting the script.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    /// Datagrams handed to the network.
    pub sent: u64,
    /// Datagrams the network threw away.
    pub dropped: u64,
    /// Extra copies the network made.
    pub duplicated: u64,
    /// Datagrams handed to a session.
    pub delivered: u64,
    /// Datagrams a session rejected (a decode, address or authentication fault).
    pub rejected: u64,
}

/// Two endpoints, the network between them, and a simulated clock.
pub struct Harness {
    /// The orchestrator endpoint.
    pub orchestrator: Session,
    /// The host endpoint.
    pub host: Session,
    /// Messages the orchestrator has received, in arrival order.
    pub orchestrator_inbox: Vec<Vec<u8>>,
    /// Messages the host has received, in arrival order.
    pub host_inbox: Vec<Vec<u8>>,
    /// What the network did.
    pub counters: Counters,
    script: Script,
    rng: u64,
    orchestrator_seq: MemorySeq,
    host_seq: MemorySeq,
    in_flight: Vec<InFlight>,
    now: u64,
    pair_key: PairKey,
    /// Channel-1 datagrams delivered to each end, for the catch-up assertion.
    pub screen_datagrams: [u64; 2],
}

impl Harness {
    /// Build a link governed by `script`.
    pub fn new(script: Script) -> Self {
        let pair_key = PairKey::from_bytes([0x42; 16]);
        let orchestrator_id = NodeId([0xA1; 16]);
        let host_id = NodeId([0xB2; 16]);
        let rng = script.seed | 1;
        Harness {
            orchestrator: Session::new(
                SessionConfig::new(
                    orchestrator_id,
                    host_id,
                    Role::Orchestrator,
                    pair_key.clone(),
                    ForwarderKey([0xC3; 32]),
                ),
                0,
            ),
            host: Session::new(
                SessionConfig::new(
                    host_id,
                    orchestrator_id,
                    Role::Host,
                    pair_key,
                    ForwarderKey([0xD4; 32]),
                ),
                0,
            ),
            orchestrator_inbox: Vec::new(),
            host_inbox: Vec::new(),
            counters: Counters::default(),
            script,
            rng,
            orchestrator_seq: MemorySeq::default(),
            host_seq: MemorySeq::default(),
            in_flight: Vec::new(),
            now: 0,
            pair_key: PairKey::from_bytes([0x42; 16]),
            screen_datagrams: [0; 2],
        }
    }

    /// The simulated clock, in milliseconds since the link started.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Run the link for `duration_ms` of simulated time.
    pub fn run_for(&mut self, duration_ms: u64) {
        let until = self.now + duration_ms;
        while self.now < until {
            self.step();
        }
    }

    /// Run until `done` holds, or `limit_ms` of simulated time has passed.
    ///
    /// Returns whether `done` became true, so a test asserts convergence rather
    /// than merely "it did not hang".
    pub fn run_until(&mut self, limit_ms: u64, mut done: impl FnMut(&Harness) -> bool) -> bool {
        let deadline = self.now + limit_ms;
        while self.now < deadline {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    /// Advance one millisecond: collect what each session wants to send, then
    /// deliver whatever has arrived.
    pub fn step(&mut self) {
        self.now += 1;
        let now = self.now;

        let from_orchestrator = self
            .orchestrator
            .outgoing(now, &mut self.orchestrator_seq)
            .expect("the orchestrator produced an unsendable datagram");
        for datagram in from_orchestrator {
            self.enqueue(End::Host, datagram);
        }
        let from_host = self
            .host
            .outgoing(now, &mut self.host_seq)
            .expect("the host produced an unsendable datagram");
        for datagram in from_host {
            self.enqueue(End::Orchestrator, datagram);
        }

        self.deliver();
    }

    /// Hand a datagram to the network, applying the script.
    fn enqueue(&mut self, to: End, bytes: Vec<u8>) {
        self.counters.sent += 1;
        if self.blacked_out(to) {
            self.counters.dropped += 1;
            return;
        }
        if self.roll() < self.script.loss_percent {
            self.counters.dropped += 1;
            return;
        }
        let mut delay = self.script.delay_ms;
        if self.roll() < self.script.reorder_percent {
            delay += self.script.reorder_delay_ms;
        }
        let duplicate = self.roll() < self.script.duplicate_percent;
        self.in_flight.push(InFlight {
            deliver_at: self.now + delay,
            to,
            bytes: bytes.clone(),
        });
        if duplicate {
            self.counters.duplicated += 1;
            self.in_flight.push(InFlight {
                deliver_at: self.now + delay + 3,
                to,
                bytes,
            });
        }
    }

    /// Deliver everything whose time has come.
    fn deliver(&mut self) {
        let now = self.now;
        let ready: Vec<InFlight> = {
            let (ready, waiting) = std::mem::take(&mut self.in_flight)
                .into_iter()
                .partition(|packet| packet.deliver_at <= now);
            self.in_flight = waiting;
            ready
        };
        for packet in ready {
            // A datagram whose delivery falls inside a blackout is lost too: the
            // network went away while it was on the wire.
            if self.blacked_out(packet.to) {
                self.counters.dropped += 1;
                continue;
            }
            self.counters.delivered += 1;
            if channel_of(&packet.bytes, &self.pair_key) == Some(CHANNEL_SCREEN) {
                self.screen_datagrams[usize::from(packet.to == End::Host)] += 1;
            }
            let accepted = match packet.to {
                End::Host => self.host.handle_datagram(&packet.bytes, now),
                End::Orchestrator => self.orchestrator.handle_datagram(&packet.bytes, now),
            };
            if accepted.is_err() {
                self.counters.rejected += 1;
                continue;
            }
            match packet.to {
                End::Host => {
                    let messages = self.host.take_messages();
                    self.host_inbox.extend(messages);
                }
                End::Orchestrator => {
                    let messages = self.orchestrator.take_messages();
                    self.orchestrator_inbox.extend(messages);
                }
            }
        }
    }

    /// Whether a blackout covering `to` is in force now.
    fn blacked_out(&self, to: End) -> bool {
        self.script.blackouts.iter().any(|(from, until, side)| {
            self.now >= *from && self.now < *until && side.is_none_or(|side| side == to)
        })
    }

    /// The next pseudo-random percentage. A plain LCG: reproducible, and the
    /// quality of the randomness is irrelevant next to the determinism.
    fn roll(&mut self) -> u32 {
        self.rng = self
            .rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.rng >> 33) % 100) as u32
    }
}

/// The channel a datagram carries.
///
/// The channel lives inside the encrypted payload, so a real forwarder could
/// never learn this — which is the point of the design. The harness can, because
/// it holds the pair key, and counting per-channel datagrams is how the
/// latest-wins catch-up is asserted to cost exactly one diff.
fn channel_of(datagram: &[u8], pair_key: &PairKey) -> Option<u8> {
    let header = OuterHeader::decode(datagram).ok()?;
    let plaintext = medulla_link::crypto::open(
        pair_key,
        header.seq,
        &header.signed_bytes(),
        &datagram[medulla_link::header::HEADER_LEN..],
    )
    .ok()?;
    Some(Packet::decode(&plaintext).ok()?.instruction.channel)
}
