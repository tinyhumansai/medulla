//! The 58-byte cleartext outer header (protocol §3).
//!
//! These are the only bytes the forwarder parses, and the only bytes it is able
//! to parse. Layout, big-endian throughout:
//!
//! ```text
//!   0  1  version   = 1
//!   1  1  flags     bit 0 = heartbeat; bits 1-7 reserved, MUST be 0
//!   2 16  src_node_id
//!  18 16  dst_node_id
//!  34  8  seq       counter with the direction bit in bit 63
//!  42 16  tag       HMAC-SHA256(forwarder_key, bytes[0..42])[0..16]
//!  58  …  payload   opaque (§4)
//! ```
//!
//! Bytes `[0..42]` are both the HMAC input and the AEAD's AAD, so a forwarder
//! that rewrote any header field would break the payload's authentication as
//! well as its own tag. §5 rule 8 forbids rewriting for exactly that reason.
//!
//! An **endpoint** does not verify `tag`: a forwarder key is per node and known
//! only to its owner and the backend, so the receiving endpoint cannot check the
//! sender's. The endpoint's authentication is the AEAD over this header as AAD.
//! [`verify_tag`] exists for the forwarder — and for tests that stand in for one.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::keys::{ForwarderKey, NodeId};

/// Protocol version carried in byte 0.
pub const VERSION: u8 = 1;

/// Total header size.
pub const HEADER_LEN: usize = 58;

/// Offset of the tag, and therefore the length of the HMAC/AAD input.
pub const TAG_OFFSET: usize = 42;

/// Length of the truncated HMAC tag.
pub const TAG_LEN: usize = 16;

/// `flags` bit 0: this datagram carries no state change (protocol §6.1).
pub const FLAG_HEARTBEAT: u8 = 0x01;

/// Maximum total datagram size (protocol §3.2).
///
/// Sized to stay inside a typical 1500-byte path MTU with room for IPv6 and
/// tunnelling, so a datagram is never fragmented: a fragmented UDP datagram is
/// dropped whole if any fragment is lost, which would defeat the loss tolerance
/// the design is built on.
pub const MAX_DATAGRAM: usize = 1400;

/// Why a datagram's header could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HeaderError {
    /// Fewer than [`HEADER_LEN`] bytes.
    #[error("datagram is {0} bytes, shorter than the {HEADER_LEN}-byte header")]
    TooShort(usize),
    /// A version this implementation does not speak.
    #[error("unsupported link protocol version {0}")]
    Version(u8),
    /// A reserved flag bit was set. Reserved bits MUST be 0 (§3), so a set bit
    /// is a peer speaking a dialect we do not know rather than noise to ignore.
    #[error("reserved flag bits set: {0:#04x}")]
    ReservedFlags(u8),
}

/// The parsed outer header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OuterHeader {
    /// Bit 0 set means heartbeat; the rest are reserved and always 0.
    pub flags: u8,
    /// Who sent it.
    pub src: NodeId,
    /// Who it is for.
    pub dst: NodeId,
    /// The §3.1 sequence, direction bit included.
    pub seq: u64,
}

impl OuterHeader {
    /// A header for a datagram carrying a state change.
    pub fn new(src: NodeId, dst: NodeId, seq: u64) -> Self {
        OuterHeader {
            flags: 0,
            src,
            dst,
            seq,
        }
    }

    /// Mark this datagram as a heartbeat (no state change).
    pub fn heartbeat(mut self) -> Self {
        self.flags |= FLAG_HEARTBEAT;
        self
    }

    /// Whether bit 0 of `flags` is set.
    pub fn is_heartbeat(&self) -> bool {
        self.flags & FLAG_HEARTBEAT != 0
    }

    /// The bytes covered by the HMAC and used as the AEAD's AAD: `[0..42]`.
    pub fn signed_bytes(&self) -> [u8; TAG_OFFSET] {
        let mut out = [0u8; TAG_OFFSET];
        out[0] = VERSION;
        out[1] = self.flags;
        out[2..18].copy_from_slice(self.src.as_bytes());
        out[18..34].copy_from_slice(self.dst.as_bytes());
        out[34..42].copy_from_slice(&self.seq.to_be_bytes());
        out
    }

    /// Encode the full 58 bytes, tagged with `key`.
    pub fn encode(&self, key: &ForwarderKey) -> [u8; HEADER_LEN] {
        let signed = self.signed_bytes();
        let mut out = [0u8; HEADER_LEN];
        out[..TAG_OFFSET].copy_from_slice(&signed);
        out[TAG_OFFSET..].copy_from_slice(&tag_for(key, &signed));
        out
    }

    /// Parse the header at the front of `datagram`.
    ///
    /// The tag is **not** checked here — see the module docs for why an endpoint
    /// cannot check it. Use [`verify_tag`] where a forwarder key is available.
    ///
    /// # Errors
    ///
    /// [`HeaderError::TooShort`], [`HeaderError::Version`], or
    /// [`HeaderError::ReservedFlags`].
    pub fn decode(datagram: &[u8]) -> Result<Self, HeaderError> {
        if datagram.len() < HEADER_LEN {
            return Err(HeaderError::TooShort(datagram.len()));
        }
        if datagram[0] != VERSION {
            return Err(HeaderError::Version(datagram[0]));
        }
        let flags = datagram[1];
        if flags & !FLAG_HEARTBEAT != 0 {
            return Err(HeaderError::ReservedFlags(flags));
        }
        let mut src = [0u8; 16];
        src.copy_from_slice(&datagram[2..18]);
        let mut dst = [0u8; 16];
        dst.copy_from_slice(&datagram[18..34]);
        let mut seq = [0u8; 8];
        seq.copy_from_slice(&datagram[34..42]);
        Ok(OuterHeader {
            flags,
            src: NodeId(src),
            dst: NodeId(dst),
            seq: u64::from_be_bytes(seq),
        })
    }
}

/// `HMAC-SHA256(key, signed)[0..16]`.
pub fn tag_for(key: &ForwarderKey, signed: &[u8]) -> [u8; TAG_LEN] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_bytes())
        .expect("HMAC-SHA256 accepts a key of any length");
    mac.update(signed);
    let full = mac.finalize().into_bytes();
    let mut tag = [0u8; TAG_LEN];
    tag.copy_from_slice(&full[..TAG_LEN]);
    tag
}

/// Whether `datagram`'s tag verifies against `key`.
///
/// The comparison is constant time (protocol §5 rule 3): a variable-time compare
/// would let an attacker recover a valid tag byte by byte from timing.
pub fn verify_tag(datagram: &[u8], key: &ForwarderKey) -> bool {
    if datagram.len() < HEADER_LEN {
        return false;
    }
    let expected = tag_for(key, &datagram[..TAG_OFFSET]);
    expected.ct_eq(&datagram[TAG_OFFSET..HEADER_LEN]).into()
}

#[cfg(test)]
#[path = "header_tests.rs"]
mod tests;
