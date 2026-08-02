//! Payload confidentiality: ChaCha20-Poly1305 under the pair key (protocol §4).
//!
//! ```text
//! payload = ChaCha20-Poly1305(key = pair_key, nonce = §4.2,
//!                             aad = header[0..42], plaintext = §4.1)
//! ```
//!
//! Two properties are worth stating because both are load-bearing.
//!
//! **The AAD is the outer header.** A recipient that decrypts successfully has
//! proved the datagram was addressed to *it*, by *that* sender, at *that*
//! sequence. A forwarder that redirected a datagram to a different node would
//! produce an authentication failure rather than a delivered message — the
//! blindness of the forwarder is enforced, not trusted.
//!
//! **The nonce carries a direction bit.** Both endpoints hold the same pair key,
//! so they must not draw nonces from the same space. Bit 63 of `seq` is set by
//! the orchestrator and clear by the host, splitting the counter in half. This
//! is mosh's construction and the reason for it.
//!
//! *Deliberate deviation from mosh:* ChaCha20-Poly1305 rather than OCB-AES128 —
//! the same AEAD role, first-class in Rust, no OCB dependency.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

use crate::keys::PairKey;

/// Bit 63 of `seq`: set = orchestrator → host, clear = host → orchestrator.
pub const DIRECTION_MASK: u64 = 1 << 63;

/// Bytes the AEAD adds to the plaintext.
pub const AEAD_OVERHEAD: usize = 16;

/// Why a payload could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// The AEAD tag did not verify: a corrupted datagram, the wrong pair key, or
    /// a header that was rewritten in flight (the AAD binding of §4).
    #[error("payload failed authentication")]
    Authentication,
}

/// Seal a plaintext for the peer.
///
/// `seq` is the full sequence **including** the direction bit, exactly as it
/// appears in the outer header. `aad` must be `header[0..42]` — the header bytes
/// up to and excluding the forwarder tag.
pub fn seal(key: &PairKey, seq: u64, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&expand_key(key)));
    let nonce = nonce_for(seq);
    cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        // The only documented failure is a plaintext longer than the cipher's
        // limit (256 GiB); a link datagram is capped at 1400 bytes by §3.2.
        .expect("ChaCha20-Poly1305 cannot fail on a datagram-sized plaintext")
}

/// Open a payload from the peer.
///
/// # Errors
///
/// [`CryptoError::Authentication`] on any tag failure. There is deliberately no
/// finer-grained error: distinguishing "wrong key" from "tampered" would leak
/// which one an attacker got wrong.
pub fn open(
    key: &PairKey,
    seq: u64,
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&expand_key(key)));
    let nonce = nonce_for(seq);
    cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Authentication)
}

/// The §4.2 nonce: four zero bytes then `seq` as a 64-bit big-endian integer,
/// direction bit included.
pub fn nonce_for(seq: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&seq.to_be_bytes());
    nonce
}

/// Widen the 128-bit pair key to the 256-bit key ChaCha20-Poly1305 takes.
///
/// The pair key is 128 bits because a human retypes it (§7.1). It is stretched
/// with SHA-256 over a domain-separated label rather than zero-padded, so the
/// cipher key depends on every bit of the pair key and the construction is not
/// silently reusable for any other purpose. The security level stays 128-bit —
/// that is the entropy the user carried — which is the intended strength.
fn expand_key(key: &PairKey) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"medulla-link/1 pair-key");
    hasher.update(key.as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
#[path = "crypto_tests.rs"]
mod tests;
