//! A loopback UDP forwarder standing in for the backend, implementing the
//! forwarder rules of `docs/host-link-protocol.md` §5 over an in-memory node
//! table seeded from the command line.
//!
//! This replaces the mock Signal server the coordination e2e harness used to
//! run: with the host link there is no relay to store envelopes in and no
//! directory to publish keys to, only a blind forwarder that moves bytes it
//! cannot read between enrolled nodes. It exists so the harness can run the
//! whole two-daemon fleet on loopback with no network and no backend.
//!
//! **It never looks at the payload.** The only bytes it parses are the 58-byte
//! cleartext header (§3); everything from offset 58 on is copied verbatim to the
//! destination (§5 rule 8). Rewriting any header field would break both the
//! AEAD's AAD binding and the receiver's nonce, so nothing is rewritten either.
//!
//! # Rules, in order (§5)
//!
//! 1. `len ≥ 58` and `version == 1` (plus the §3.2 size cap, checked first).
//! 2. `src_node_id` resolves to a known node.
//! 3. `tag` verifies against that node's forwarder key, in constant time.
//! 4. Replay window: `highest_seq` plus a 64-bit bitmap of the sequences below it.
//! 5. Rebind **only** when `seq > highest_seq` — a captured datagram replayed
//!    from another address must not steal a node's binding.
//! 6. `dst_node_id` is a node in the **same team**; cross-team is dropped and
//!    counted as a security event.
//! 7. `dst` has a live binding, or the datagram is dropped (SSP retransmits).
//! 8. Forward the bytes verbatim.
//!
//! # What this mock deliberately does not do
//!
//! §5.1's rate limiting (a per-source-address bucket before the HMAC, a per-node
//! bucket after it) and the bounded unresolved-node-id cache are omitted. They
//! defend against an unauthenticated flood, which a loopback harness with two
//! enrolled endpoints cannot produce; a real forwarder MUST implement them.
//! Bindings here also never expire — a harness run is minutes long, and a TTL
//! that never fires is worse than an honest absence.
//!
//! # Usage
//!
//! ```text
//! mock_link_forwarder --bind 127.0.0.1:7000 \
//!     --node <node-id-hex>:<forwarder-key-hex>:<team> \
//!     --node <node-id-hex>:<forwarder-key-hex>:<team>
//! ```
//!
//! `--bind` defaults to `127.0.0.1:0`. The first stdout line is
//! `mock_link_forwarder listening on <addr>`, which is what the harness scrapes.
//! Every forwarded datagram logs one `forward` line and every drop logs one
//! `drop` line with its reason, so a failing run is diagnosable from the log
//! alone — with no more information than the forwarder is entitled to have.

use std::collections::HashMap;
use std::net::SocketAddr;

use medulla_link::header::{verify_tag, OuterHeader, HEADER_LEN, MAX_DATAGRAM};
use medulla_link::keys::{ForwarderKey, NodeId};
use tokio::net::UdpSocket;

/// One enrolled node: what the backend knows about it, and where it was last
/// heard from.
struct Node {
    /// The outer-header HMAC key issued at enrollment (§7.2).
    key: ForwarderKey,
    /// The tenancy boundary; a datagram never crosses it (§5 rule 6).
    team: String,
    /// Where datagrams for this node go, learned from its own traffic (§5 rule 5).
    binding: Option<SocketAddr>,
    /// Highest sequence accepted from this node.
    highest_seq: u64,
    /// Bit `i` marks sequence `highest_seq - 1 - i` as already seen.
    seen: u64,
}

impl Node {
    /// Whether `seq` is inside the replay window and not already seen (§5 rule 4),
    /// and whether it advances the window (§5 rule 5).
    ///
    /// Marks the sequence as seen when it admits it, so a duplicate is refused
    /// the second time.
    fn admit(&mut self, seq: u64) -> Admission {
        if seq > self.highest_seq {
            let shift = seq - self.highest_seq;
            // The old highest is now `shift` below the new one, so it takes bit
            // `shift - 1`. A jump of 64 or more clears the window entirely.
            self.seen = if shift >= 64 {
                0
            } else {
                (self.seen << shift) | (1u64 << (shift - 1))
            };
            self.highest_seq = seq;
            return Admission::Advance;
        }
        let behind = self.highest_seq - seq;
        // `behind == 0` is the highest sequence itself, which has been seen.
        if behind == 0 || behind >= 64 {
            return Admission::Refuse;
        }
        let bit = 1u64 << (behind - 1);
        if self.seen & bit != 0 {
            return Admission::Refuse;
        }
        self.seen |= bit;
        Admission::Accept
    }
}

/// What the replay window says about one datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admission {
    /// Fresh and strictly ahead: forward it, and rebind the source.
    Advance,
    /// Inside the window and not seen before: forward it, but do not rebind.
    Accept,
    /// A replay, or older than the window: drop it.
    Refuse,
}

/// The forwarder's whole world: the node table it was seeded with.
struct Forwarder {
    nodes: HashMap<NodeId, Node>,
    /// Cross-team attempts, which §5 rule 6 calls a security event rather than
    /// noise.
    cross_team: u64,
}

impl Forwarder {
    /// Apply §5 to one datagram, returning where to send it verbatim.
    fn route(&mut self, datagram: &[u8], from: SocketAddr) -> Result<SocketAddr, String> {
        // §3.2 size cap, and §5 rule 1. Both are free, so they come first.
        if datagram.len() > MAX_DATAGRAM {
            return Err(format!("oversized: {} bytes", datagram.len()));
        }
        if datagram.len() < HEADER_LEN {
            return Err(format!("short: {} bytes", datagram.len()));
        }
        let header = OuterHeader::decode(datagram).map_err(|e| format!("header: {e}"))?;

        // Rule 2: a source we do not know is dropped without further work.
        let source = self
            .nodes
            .get_mut(&header.src)
            .ok_or_else(|| format!("unknown src {}", header.src))?;

        // Rule 3: constant-time tag comparison under that node's key.
        if !verify_tag(datagram, &source.key) {
            return Err(format!("bad tag from {}", header.src));
        }

        // Rules 4 and 5.
        match source.admit(header.seq) {
            Admission::Advance => source.binding = Some(from),
            Admission::Accept => {}
            Admission::Refuse => {
                return Err(format!("replayed seq {} from {}", header.seq, header.src))
            }
        }
        let team = source.team.clone();

        // Rule 6: same team, or a counted drop.
        let destination = self
            .nodes
            .get(&header.dst)
            .ok_or_else(|| format!("unknown dst {}", header.dst))?;
        if destination.team != team {
            self.cross_team += 1;
            return Err(format!(
                "cross-team {} → {} (count {})",
                header.src, header.dst, self.cross_team
            ));
        }

        // Rule 7: no binding yet means the peer has never been heard from. The
        // sender's SSP will retransmit, so dropping costs a round trip, not a
        // message.
        destination
            .binding
            .ok_or_else(|| format!("no binding for dst {}", header.dst))
    }
}

/// Parse `--node <node-id-hex>:<forwarder-key-hex>:<team>`.
fn parse_node(spec: &str) -> Result<(NodeId, Node), String> {
    let mut parts = spec.split(':');
    let (Some(id), Some(key), Some(team)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!(
            "--node wants <node-id-hex>:<forwarder-key-hex>:<team>, got {spec:?}"
        ));
    };
    if parts.next().is_some() {
        return Err(format!("--node has too many fields: {spec:?}"));
    }
    let id: [u8; 16] = decode_hex(id, 16)?
        .try_into()
        .expect("decode_hex checked the length");
    let key: [u8; 32] = decode_hex(key, 32)?
        .try_into()
        .expect("decode_hex checked the length");
    Ok((
        NodeId(id),
        Node {
            key: ForwarderKey(key),
            team: team.to_string(),
            binding: None,
            highest_seq: 0,
            seen: 0,
        },
    ))
}

/// Decode exactly `want` bytes of hex.
fn decode_hex(text: &str, want: usize) -> Result<Vec<u8>, String> {
    if text.len() != want * 2 {
        return Err(format!(
            "expected {} hex characters, got {}",
            want * 2,
            text.len()
        ));
    }
    (0..want)
        .map(|index| {
            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
                .map_err(|_| format!("not hex: {text:?}"))
        })
        .collect()
}

/// The parsed command line.
struct Args {
    bind: String,
    nodes: HashMap<NodeId, Node>,
}

fn parse_args() -> Result<Args, String> {
    let mut bind = "127.0.0.1:0".to_string();
    let mut nodes = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bind" => bind = it.next().ok_or("--bind needs a value")?,
            "--node" => {
                let (id, node) = parse_node(&it.next().ok_or("--node needs a value")?)?;
                nodes.insert(id, node);
            }
            other => return Err(format!("unexpected argument: {other}")),
        }
    }
    if nodes.is_empty() {
        return Err("no --node given: a forwarder with an empty table drops everything".into());
    }
    Ok(Args { bind, nodes })
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("mock_link_forwarder: {err}");
        std::process::exit(2);
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let socket = UdpSocket::bind(&args.bind)
        .await
        .map_err(|e| format!("could not bind {}: {e}", args.bind))?;
    let address = socket
        .local_addr()
        .map_err(|e| format!("could not read the bound address: {e}"))?;
    for (id, node) in &args.nodes {
        eprintln!("mock_link_forwarder: node {id} team={}", node.team);
    }
    // The harness scrapes this line, so it is the first thing on stdout.
    println!("mock_link_forwarder listening on {address}");

    let mut forwarder = Forwarder {
        nodes: args.nodes,
        cross_team: 0,
    };
    let mut buffer = vec![0u8; MAX_DATAGRAM * 2];
    loop {
        let (len, from) = socket
            .recv_from(&mut buffer)
            .await
            .map_err(|e| format!("recv failed: {e}"))?;
        let datagram = &buffer[..len];
        match forwarder.route(datagram, from) {
            Ok(destination) => {
                // Verbatim (§5 rule 8). The header is logged because the
                // forwarder is entitled to it; the payload is neither logged nor
                // inspected, because it is not.
                let header = OuterHeader::decode(datagram).expect("route decoded this header");
                let _ = socket.send_to(datagram, destination).await;
                println!(
                    "forward src={} dst={} seq={} bytes={} kind={}",
                    header.src,
                    header.dst,
                    header.seq,
                    len,
                    if header.is_heartbeat() {
                        "heartbeat"
                    } else {
                        "state"
                    }
                );
            }
            Err(reason) => println!("drop from={from} reason={reason}"),
        }
    }
}
