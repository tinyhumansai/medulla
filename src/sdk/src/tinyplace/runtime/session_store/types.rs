//! Data types for the `session_store` module.
#[allow(unused_imports)]
use super::*;
/// A filesystem-backed [`SessionStore`]. All ratchet/pre-key state for one
/// identity lives in a single JSON file (`<dir>/signal/<address>.json`), written
/// atomically (temp file + rename) with `0600` permissions. The long-term
/// identity X25519 key pair is derived from the wallet seed and supplied at
/// construction — it is never written to disk.
///
/// This store keeps no in-memory cache: every operation reads the file fresh and
/// each mutation rewrites it atomically, so it stays coherent when another
/// process on the same wallet advances the ratchet. It does **not** lock: callers
/// sharing one wallet must serialize their operations (as the tinyplace machine
/// bus does).
pub struct FileSessionStore {
    pub(super) path: PathBuf,
    pub(super) identity_key_pair: X25519KeyPair,
}
