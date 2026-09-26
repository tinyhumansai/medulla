//! Talking to a peer with no forwarder in the path (the SSH-bootstrapped case).
//!
//! The enrolled link reaches a peer through the backend forwarder, which does
//! three jobs: it routes on the cleartext header, it authenticates that header
//! under a per-node forwarder key, and it relearns node addresses so roaming is
//! free (§5 rule 5). A direct link has no third party, so each job needs its own
//! answer:
//!
//! | Forwarder job | Direct-mode answer |
//! |---|---|
//! | Routing | Nothing to route: one socket, one peer. |
//! | Outer-header HMAC | A **path key** derived from the pair key, so both ends hold it. |
//! | Roaming and replay defence | Highest-sequence address learning ([`learn`]). |
//!
//! # Why the outer tag becomes useful here
//!
//! Under a forwarder an endpoint cannot verify the tag: the forwarder key is per
//! node and known only to its owner and the backend, so the receiver has never
//! held the sender's (see [`crate::header`]). The endpoint's real authentication
//! is the AEAD over the header as AAD, and that is unchanged here.
//!
//! With no forwarder, both endpoints can derive the *same* outer key from the
//! pair key they already share — so the tag can be checked, and it becomes a
//! cheap constant-time reject of off-path junk *before* ChaCha20-Poly1305. That
//! is the one useful thing the forwarder used to do on our behalf, recovered.
//!
//! It is not a new secret and not a second security boundary: it is derived from
//! the pair key under its own label, exactly as the AEAD key is
//! ([`crate::crypto`]). Anything that can forge it could already decrypt.

use std::net::SocketAddr;

use crate::keys::{ForwarderKey, PairKey};
use crate::transport::Session;

/// The outer-header key for a direct link, derived from the pair key.
///
/// Domain-separated from the AEAD's `"medulla-link/1 pair-key"` label so the two
/// derivations can never collide, and so this construction is not silently
/// reusable for anything else.
pub fn path_key(pair_key: &PairKey) -> ForwarderKey {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"medulla-link/1 direct-path");
    hasher.update(pair_key.as_bytes());
    ForwarderKey(hasher.finalize().into())
}

/// Where a link's datagrams go.
#[derive(Debug, Clone, Copy)]
pub(super) enum Route {
    /// Through the forwarder: one fixed address, and datagrams from anything
    /// else are dropped. The forwarder relearns peer addresses for us.
    Forwarder(SocketAddr),
    /// Straight to the peer, at an address that starts where the bootstrap said
    /// and moves only under [`learn`].
    Direct(SocketAddr),
}

impl Route {
    /// Whether a datagram from `from` is worth parsing at all.
    ///
    /// Forwarded mode keeps its exact-match check: nothing but the forwarder can
    /// legitimately send to us, so anything else is junk. Direct mode accepts any
    /// source and defers the decision to authentication, because "a datagram from
    /// an address we have not seen before" is precisely what roaming looks like.
    pub(super) fn accepts(&self, from: SocketAddr) -> bool {
        match self {
            Route::Forwarder(address) => *address == from,
            Route::Direct(_) => true,
        }
    }

    /// The address to send to.
    pub(super) fn destination(&self) -> SocketAddr {
        match self {
            Route::Forwarder(address) | Route::Direct(address) => *address,
        }
    }
}

/// Adopt `from` as the peer's address, if the datagram that arrived from it was
/// both authentic and fresh.
///
/// Both halves are load-bearing, and the freshness half is the one that is easy
/// to leave out.
///
/// Authentication alone is not enough. Every datagram stays authentic forever, so
/// an off-path attacker who captures one and replays it from their own address
/// would move the flow there. They cannot send through it — they hold no key — so
/// it is a blackhole rather than a hijack, but it is a denial of service that
/// persists: the victim's next transmission goes to the attacker's address, is
/// never acknowledged, and nothing arrives to correct the mistake.
///
/// So an address is adopted only from a datagram that *advances* the highest
/// sequence this session has accepted. A replay cannot, by definition. This is
/// mosh's rule, and the persisted sequence reservation (§3.1) keeps it holding
/// across a restart: sequences never go backwards, so a restarted peer's first
/// datagram still advances the watermark.
///
/// Called only after [`Session::handle_datagram`] has returned successfully, so
/// "authentic" is already established by the time this runs.
pub(super) fn learn(route: &mut Route, session: &Session, from: SocketAddr) {
    let Route::Direct(address) = route else {
        return;
    };
    if *address == from || !session.peer_seq_advanced() {
        return;
    }
    *route = Route::Direct(from);
}

#[cfg(test)]
#[path = "direct_tests.rs"]
mod tests;
