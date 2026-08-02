//! The pair key and its human-transcribable encoding (protocol §7.1).
//!
//! The pair key is the 128-bit end-to-end secret. It is generated on the
//! orchestrator, read aloud or carried to the host, and typed in there — so its
//! encoding is optimised for a human retyping it, not for compactness:
//!
//! ```text
//! payload   = 128-bit key ‖ 12-bit checksum        (140 bits)
//! checksum  = SHA-256(key)[0..12 bits]
//! encoding  = Crockford base32, no padding          (28 chars)
//! display   = 7 groups of 4, hyphen-separated
//! ```
//!
//! Crockford base32 excludes `I`, `L`, `O` and `U`, and [`PairKey::decode`]
//! folds the confusable characters, so `0`/`O` and `1`/`I`/`L` typos resolve
//! rather than fail. The checksum catches everything else at entry — where the
//! user can see what they mistyped — instead of surfacing later as an
//! unexplained decrypt failure.

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// Crockford's base32 alphabet: no `I`, `L`, `O` or `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Encoded length: 140 bits / 5 bits per character.
const ENCODED_CHARS: usize = 28;

/// Characters per hyphen-separated display group.
const GROUP: usize = 4;

/// Why a typed pair key was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairKeyError {
    /// Not 28 significant characters once separators are stripped.
    #[error("pair key must be {ENCODED_CHARS} characters, got {0}")]
    Length(usize),
    /// A character that is not in the Crockford alphabet, even after folding.
    #[error("pair key contains an invalid character: {0:?}")]
    InvalidCharacter(char),
    /// The 12-bit checksum did not match — almost certainly a typo.
    #[error("pair key checksum does not match; check for a mistyped character")]
    Checksum,
}

/// The 128-bit end-to-end key shared by exactly two endpoints.
///
/// `Debug` is redacted and there is no `Display`: printing a pair key is a
/// deliberate act that goes through [`PairKey::encode`].
#[derive(Clone, PartialEq, Eq)]
pub struct PairKey([u8; 16]);

impl PairKey {
    /// Mint a fresh random pair key. This happens on the orchestrator, once per
    /// host, and the result is shown to the user (protocol §7.1).
    pub fn generate() -> Self {
        let mut bytes = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
        PairKey(bytes)
    }

    /// Wrap raw key bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        PairKey(bytes)
    }

    /// The raw key, for the AEAD.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Encode for display: 7 hyphen-separated groups of 4 Crockford characters,
    /// checksum included.
    pub fn encode(&self) -> String {
        let raw = self.encode_compact();
        let mut out = String::with_capacity(ENCODED_CHARS + ENCODED_CHARS / GROUP - 1);
        for (index, ch) in raw.chars().enumerate() {
            if index > 0 && index.is_multiple_of(GROUP) {
                out.push('-');
            }
            out.push(ch);
        }
        out
    }

    /// Encode without the display grouping — 28 characters, no separators.
    pub fn encode_compact(&self) -> String {
        let checksum = checksum_of(&self.0);
        // 140 bits, big-endian: the key then the checksum's low 12 bits.
        let mut bits = Vec::with_capacity(140);
        for byte in self.0 {
            for offset in (0..8).rev() {
                bits.push((byte >> offset) & 1 == 1);
            }
        }
        for offset in (0..12).rev() {
            bits.push((checksum >> offset) & 1 == 1);
        }
        bits.chunks(5)
            .map(|chunk| {
                let value = chunk
                    .iter()
                    .fold(0u8, |acc, bit| (acc << 1) | u8::from(*bit));
                ALPHABET[value as usize] as char
            })
            .collect()
    }

    /// Decode a key a human typed.
    ///
    /// Hyphens, spaces and case are ignored; `O` folds to `0` and `I`/`L` fold
    /// to `1`. A wrong length, an unfoldable character, or a checksum mismatch
    /// is an error here rather than an opaque decrypt failure later.
    pub fn decode(text: &str) -> Result<Self, PairKeyError> {
        let mut values = Vec::with_capacity(ENCODED_CHARS);
        for ch in text.chars() {
            if ch == '-' || ch.is_whitespace() {
                continue;
            }
            values.push(decode_char(ch).ok_or(PairKeyError::InvalidCharacter(ch))?);
        }
        if values.len() != ENCODED_CHARS {
            return Err(PairKeyError::Length(values.len()));
        }

        let mut bits = Vec::with_capacity(140);
        for value in values {
            for offset in (0..5).rev() {
                bits.push((value >> offset) & 1 == 1);
            }
        }
        let mut key = [0u8; 16];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = bits[index * 8..index * 8 + 8]
                .iter()
                .fold(0u8, |acc, bit| (acc << 1) | u8::from(*bit));
        }
        let found = bits[128..140]
            .iter()
            .fold(0u16, |acc, bit| (acc << 1) | u16::from(*bit));
        if found != checksum_of(&key) {
            return Err(PairKeyError::Checksum);
        }
        Ok(PairKey(key))
    }
}

impl fmt::Debug for PairKey {
    /// Never prints the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairKey(<redacted>)")
    }
}

impl Serialize for PairKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.encode())
    }
}

impl<'de> Deserialize<'de> for PairKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        PairKey::decode(&text).map_err(D::Error::custom)
    }
}

/// The top 12 bits of `SHA-256(key)`, right-aligned in a `u16`.
fn checksum_of(key: &[u8; 16]) -> u16 {
    let digest = Sha256::digest(key);
    (u16::from(digest[0]) << 4) | u16::from(digest[1] >> 4)
}

/// Map one typed character to its 5-bit value, folding the confusables
/// Crockford's alphabet deliberately omits. `U` stays invalid: it is excluded
/// from the alphabet and folding it would guess at the user's intent.
fn decode_char(ch: char) -> Option<u8> {
    let upper = ch.to_ascii_uppercase();
    let folded = match upper {
        'O' => '0',
        'I' | 'L' => '1',
        other => other,
    };
    ALPHABET
        .iter()
        .position(|candidate| *candidate as char == folded)
        .map(|index| index as u8)
}
