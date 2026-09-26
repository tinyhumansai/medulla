//! Bringing a remote host up, and telling the client how to reach it.
//!
//! This is mosh's bootstrap, and it is deliberately the same shape. The SSH
//! channel is already authenticated and encrypted, so it is the right place —
//! and the only place we get for free — to hand over a fresh key. One line goes
//! to stdout, SSH closes, and everything after that is UDP straight to this
//! process.
//!
//! ```text
//!   MEDULLA-CONNECT v1 port=<u16> node=<32 hex> key=<XXXX-XXXX-…>
//! ```
//!
//! # The key is minted here, never passed in
//!
//! The client never sends a secret to the far side; the far side mints one and
//! sends it back up the SSH pipe. That is not a stylistic choice: a key passed
//! as an argument would appear in this machine's `ps` output for every user on
//! it, for as long as the daemon ran. The node id and port do travel in argv,
//! because neither is a secret — a node id rides in cleartext outer headers by
//! design.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use medulla_link::keys::{self, EnrolledPeer, ForwarderKey, NodeId, NodeState, PairKey, Role};

/// The sentinel a client scans stdout for.
///
/// Scanned for rather than expected on the first line, because a login shell
/// prints whatever it likes first — a MOTD, a `Last login:` banner, `stty`
/// noise — and none of that is an error.
pub const CONNECT_SENTINEL: &str = "MEDULLA-CONNECT v1";

/// Where one client's daemon keeps its state, given a medulla home.
///
/// Two things are deliberate about this path.
///
/// It is not `<home>/link`, which the enrolled forwarder identity uses:
/// `keys::acquire` takes an advisory lock for the life of the process, so a
/// machine running both a normal `medulla` and this would deadlock on one
/// directory. They are also different identities — one enrolled with the
/// backend, one minted here — and sharing a file would let a re-bootstrap
/// clobber an enrollment.
///
/// And it is keyed by the *client*, which is what makes reconnecting work. A
/// daemon outlives the client that started it, on purpose — that is how a remote
/// session survives closing the laptop. So the next bootstrap finds one already
/// running, and the two must agree on which one: same client, same directory,
/// same daemon, same sessions ([`running_instance`]). A different client gets its
/// own directory and its own daemon, and cannot see the first one's sessions.
pub fn client_dir(home: &Path, client: NodeId) -> PathBuf {
    home.join("remote").join("hosts").join(client.to_string())
}

/// Where a client's daemon records how to reach it.
pub fn instance_path(home: &Path, client: NodeId) -> PathBuf {
    client_dir(home, client).join("instance.json")
}

/// The link identity inside a client's directory.
pub fn link_dir(dir: &Path) -> PathBuf {
    dir.join("link")
}

/// What a running daemon leaves behind so the next bootstrap can find it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Instance {
    /// The UDP port it is listening on.
    pub port: u16,
    /// Its node id.
    pub node_id: NodeId,
}

/// The connect line for a daemon already serving `client`, if one is running.
///
/// Called when the identity lock is held, which means a daemon for this client
/// already has it. Rather than fail — or, worse, start a second daemon that the
/// client could reach but that owns none of its sessions — this reads back what
/// the running one published and hands the client the same door it used before.
///
/// The pair key is the *existing* one rather than a fresh one, and that is safe
/// for the reason the persisted sequence reservation exists (§3.1): sequences
/// never rewind across restarts, so reusing a key cannot reuse a nonce.
///
/// `None` when there is no instance file, or it does not parse, or it names no
/// key for this client — all of which mean "no usable instance", so the caller
/// should report the lock rather than pretend.
pub fn running_instance(home: &Path, client: NodeId) -> Option<ConnectLine> {
    let dir = client_dir(home, client);
    let instance: Instance =
        serde_json::from_str(&std::fs::read_to_string(instance_path(home, client)).ok()?).ok()?;
    let state = keys::read_node_state(&keys::node_path(&link_dir(&dir))).ok()?;
    let pair_key = state
        .peers
        .iter()
        .find(|peer| peer.node_id == client)
        .map(|peer| peer.pair_key.clone())?;
    Some(ConnectLine {
        port: instance.port,
        node_id: instance.node_id,
        pair_key,
    })
}

/// Record how to reach the daemon now serving `client`.
///
/// # Errors
///
/// When the file cannot be written — which is fatal at startup, because a daemon
/// nobody can find again is one whose sessions are stranded.
pub fn write_instance(home: &Path, client: NodeId, instance: &Instance) -> std::io::Result<()> {
    let path = instance_path(home, client);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(instance)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
    )
}

/// What a bootstrapped host tells its client.
#[derive(Debug, Clone)]
pub struct ConnectLine {
    /// The UDP port this daemon is listening on.
    pub port: u16,
    /// This daemon's node id.
    pub node_id: NodeId,
    /// The pair key minted for this client.
    pub pair_key: PairKey,
}

impl ConnectLine {
    /// Render the line for stdout.
    ///
    /// The pair key uses [`PairKey::encode`] — the checksummed, human-carriable
    /// form — so a line mangled by a terminal fails the checksum rather than
    /// producing a key that silently decrypts nothing.
    pub fn render(&self) -> String {
        format!(
            "{CONNECT_SENTINEL} port={} node={} key={}",
            self.port,
            self.node_id,
            self.pair_key.encode()
        )
    }

    /// Parse a line the far side printed, or `None` when this is not one.
    ///
    /// Tolerant of anything before the sentinel on the same line, and of field
    /// order, because what produced it is a login shell rather than a protocol.
    pub fn parse(line: &str) -> Option<Self> {
        let start = line.find(CONNECT_SENTINEL)?;
        let mut port = None;
        let mut node_id = None;
        let mut pair_key = None;
        for field in line[start + CONNECT_SENTINEL.len()..].split_whitespace() {
            let (name, value) = field.split_once('=')?;
            match name {
                "port" => port = value.parse::<u16>().ok(),
                "node" => node_id = NodeId::from_hex(value),
                "key" => pair_key = PairKey::decode(value).ok(),
                // Unknown fields are skipped rather than refused, so a newer
                // daemon may add one without breaking an older client.
                _ => {}
            }
        }
        Some(ConnectLine {
            port: port?,
            node_id: node_id?,
            pair_key: pair_key?,
        })
    }
}

/// A hold on the enroll-then-connect window for one client.
///
/// Exists because the identity lock cannot cover that window itself:
/// [`enroll_client`] must *release* `node.lock` so `Link::connect` can acquire
/// it, and between those two moments the identity is unlocked. A second
/// bootstrap arriving in that gap would enroll a fresh pair key over the one the
/// first daemon is about to start with — leaving that daemon running under a key
/// `node.json` no longer records, so the next reconnect reads the wrong key and
/// the daemon becomes unreachable without killing it by hand.
///
/// Held across both steps, the gap closes. A second bootstrapper waits, then
/// finds `node.lock` held by the now-running daemon, and takes the reuse path —
/// which writes no key, because [`enroll_client`] acquires before it writes.
///
/// Dropped by the OS if the holder dies, like every other lock here, so a
/// crashed bootstrap cannot wedge the next one.
pub struct BootstrapLock {
    _file: std::fs::File,
}

/// Take the bootstrap hold for `dir`, waiting for any other bootstrap to finish.
///
/// Blocking rather than `try_lock`: the contended case is another bootstrap of
/// this same client mid-handoff, which is short and worth waiting out. Failing
/// instead would turn a race into an error the operator has to retry manually.
///
/// # Errors
///
/// When the directory or lock file cannot be created or locked.
pub fn hold_bootstrap(dir: &Path) -> std::io::Result<BootstrapLock> {
    use fs2::FileExt;
    std::fs::create_dir_all(dir)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("bootstrap.lock"))?;
    file.lock_exclusive()?;
    Ok(BootstrapLock { _file: file })
}

/// Open or mint this host's direct-link identity and enroll `client` on it.
///
/// The identity persists across bootstraps so a returning client talks to a host
/// whose node id has not changed. The *pair key* is fresh every time: keys are
/// per client by construction, and a second client must not inherit the first
/// one's.
///
/// # Errors
///
/// When the identity directory cannot be created, opened, or locked — the last
/// meaning another daemon already holds it, which the caller handles by reusing
/// that instance rather than starting a second one.
pub fn enroll_client(
    dir: &Path,
    client: NodeId,
) -> Result<(NodeId, PairKey), medulla_link::keys::KeyError> {
    std::fs::create_dir_all(dir)?;
    let pair_key = PairKey::generate();
    let node = keys::acquire_or_create(dir, || NodeState {
        version: 1,
        node_id: NodeId::generate(),
        // The host half of the direction bit (§4.2). The client that dialled in
        // is the orchestrator, which is what makes the two nonce spaces
        // disjoint.
        role: Role::Host,
        pair_key: pair_key.clone(),
        // Never used on this path — the direct link derives its outer key from
        // the pair key instead — but the file's schema has the field, and a
        // zeroed one would be indistinguishable from a corrupt read.
        forwarder_key: ForwarderKey::generate(),
        forwarder_endpoint: String::new(),
        peer_node_id: client,
        peers: Vec::new(),
        seq_reservation: 1,
    })?;
    let node_id = node.state.node_id;

    // Re-read rather than editing `node.state`, and this is load-bearing.
    // `acquire_or_create` builds a `ReservedSeq`, which *always* reserves a
    // fresh block and writes a higher `seq_reservation` to disk — while
    // `node.state` still holds the value from before that write. Writing the
    // in-memory copy back would rewind the reservation, handing out sequences a
    // previous run may already have used, which is nonce reuse under one AEAD
    // key (§3.1). Reading the file back picks up the reservation the source just
    // persisted. The lock is still held, so nothing else can be writing.
    let mut state = keys::read_node_state(&node.path)?;
    // Replace this client's entry rather than appending, so a client that
    // re-bootstraps ends up with one key rather than accumulating stale ones.
    state.peers.retain(|peer| peer.node_id != client);
    state.peers.push(EnrolledPeer {
        node_id: client,
        pair_key: pair_key.clone(),
    });
    state.peer_node_id = client;
    state.pair_key = pair_key.clone();
    keys::write_node_state(&node.path, &state)?;
    // Released here on purpose: `Link::connect` acquires the same directory, and
    // it must find the peer this just wrote.
    drop(node);
    Ok((node_id, pair_key))
}

/// The address a daemon should bind, given an optional operator-chosen port.
///
/// `0` lets the OS choose, which is the ordinary case: the port is reported back
/// on the connect line, so nothing depends on it being predictable. A fixed port
/// exists for the case where a firewall rule has to name one in advance.
///
/// Prefers `[::]`, which on a dual-stack host also accepts IPv4-mapped clients —
/// so one socket serves both, and a host reachable only over IPv6 works at all.
/// A hard-coded `0.0.0.0` made such a host bootstrap successfully over SSH and
/// then be unreachable over UDP, which is the worst shape a failure can take.
/// Dual-stack itself is not this function's job: `Link::connect`
/// (`medulla_link::link::bind_dual_stack`) explicitly disables `IPV6_V6ONLY`
/// on whatever `[::]` address this returns, since the OS default is not
/// dual-stack everywhere the bind below succeeds. This function only decides
/// *which* address to hand it.
///
/// Probed rather than assumed: `bindv6only` is not universally off, and a host
/// with IPv6 disabled cannot bind `[::]` whatever the default says. Falling back
/// to `0.0.0.0` keeps those working exactly as before.
pub fn bind_address(port: u16) -> SocketAddr {
    match std::net::UdpSocket::bind("[::]:0") {
        Ok(_probe) => SocketAddr::from(([0u16; 8], port)),
        Err(_) => SocketAddr::from(([0, 0, 0, 0], port)),
    }
}

/// An address the daemon can point a not-yet-known peer at.
///
/// A direct link needs *an* endpoint at construction, but the daemon has not
/// heard from its client yet and learns the real address from the first datagram
/// that authenticates. Nothing is ever sent here — but it must share the socket's
/// family, because the link resolves an endpoint against the bind's family and
/// would otherwise refuse to start.
pub fn unrouted_placeholder(bind: SocketAddr) -> String {
    match bind.is_ipv4() {
        true => "127.0.0.1:9".to_string(),
        false => "[::1]:9".to_string(),
    }
}

/// Join a host and port into an endpoint, bracketing an IPv6 literal.
///
/// `format!("{host}:{port}")` is wrong for `::1` — it produces `::1:9`, which
/// parses as a different address entirely. Only a bare literal needs the
/// brackets; a hostname or an already-bracketed literal is left alone.
pub fn join_endpoint(host: &str, port: u16) -> String {
    let host = host.trim();
    let needs_brackets = host.parse::<std::net::Ipv6Addr>().is_ok() && !host.starts_with('[');
    match needs_brackets {
        true => format!("[{host}]:{port}"),
        false => format!("{host}:{port}"),
    }
}

/// Leave the SSH session that started this daemon.
///
/// Called after the connect line is printed and flushed, and it is what makes a
/// remote session outlive the client that opened it — the property the whole
/// design rests on.
///
/// Two things happen, and both are needed:
///
/// - **`setsid`** puts the daemon in its own session with no controlling
///   terminal. Without it the daemon is still the foreground command of an SSH
///   session, and when that session ends the daemon is signalled with it — so
///   every "persistent" session dies the moment the bootstrapping client goes
///   away, which is precisely what it must not do.
/// - **stdio to `/dev/null`** lets `ssh` see EOF and exit. Without it the client
///   holds an `ssh` process open forever per bootstrap, and those accumulate.
///
/// This is mosh's arrangement, reached without a `fork`: forking a process with
/// a running tokio runtime is not safe, and `setsid` alone achieves the
/// detachment because a command `sshd` runs is not already a session leader.
///
/// Failures are reported, never fatal. A daemon that could not detach still
/// serves; it is merely tied to the SSH session's lifetime, which is strictly
/// better than refusing to start.
#[cfg(unix)]
pub fn detach_from_ssh() -> Result<(), String> {
    // SAFETY: `setsid` and `dup2` are async-signal-safe libc calls with no
    // preconditions beyond valid descriptors, and nothing else in this process
    // is touching fds 0-2 at this point — the connect line is already flushed.
    unsafe {
        if libc::setsid() == -1 {
            // Already a session leader is harmless and means there is nothing to
            // leave; anything else is worth reporting.
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EPERM) {
                return Err(format!("setsid: {error}"));
            }
        }
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if null == -1 {
            return Err(format!("/dev/null: {}", std::io::Error::last_os_error()));
        }
        for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
            if libc::dup2(null, fd) == -1 {
                return Err(format!("dup2: {}", std::io::Error::last_os_error()));
            }
        }
        if null > libc::STDERR_FILENO {
            libc::close(null);
        }
    }
    Ok(())
}

/// No detach off unix: there is no `setsid`, and no SSH-started daemon either.
#[cfg(not(unix))]
pub fn detach_from_ssh() -> Result<(), String> {
    Ok(())
}
