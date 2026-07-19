//! Agent identity bootstrap: load-or-mint the 32-byte Ed25519 seed backing the
//! tiny.place signer and persist it to the tinyplace CLI config file.

use std::collections::HashMap;
use std::path::Path;

use tinyplace::LocalSigner;

use super::super::config::{load_config, write_config, TinyPlaceConfig};
use super::types::{RuntimeError, RuntimeResult};

/// Load or create the agent identity.
///
/// A 32-byte Ed25519 seed is resolved from `TINYPLACE_SECRET_KEY` (hex) in `env`,
/// then the config file's `secretKey`. When neither is set, a fresh seed is
/// generated and persisted to `config_path` (atomic, `0600`). Returns the signer
/// and the (possibly updated) config. The config's `secret_key` always reflects
/// the seed in use.
pub fn load_or_create_identity(
    config_path: &Path,
    env: &HashMap<String, String>,
) -> RuntimeResult<(LocalSigner, TinyPlaceConfig)> {
    let mut config = load_config(config_path);

    let from_env = env
        .get("TINYPLACE_SECRET_KEY")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let existing = from_env.or_else(|| {
        config
            .secret_key
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });

    if let Some(hex) = existing {
        let seed = decode_seed_hex(&hex)?;
        let signer = LocalSigner::from_seed(&seed)?;
        config.secret_key = Some(hex);
        return Ok((signer, config));
    }

    // No key anywhere: mint one and persist it.
    let signer = LocalSigner::generate();
    let hex = encode_seed_hex(&signer.seed());
    config.secret_key = Some(hex);
    write_config(config_path, &config)?;
    Ok((signer, config))
}

/// Hex-encode a 32-byte seed as 64 lowercase hex chars.
fn encode_seed_hex(seed: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in seed {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Decode a 64-char hex string into a 32-byte seed, rejecting wrong lengths and
/// non-hex input.
fn decode_seed_hex(hex: &str) -> RuntimeResult<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(RuntimeError::Invalid(format!(
            "secret key must be 64 hex chars (a 32-byte seed), got {}",
            hex.len()
        )));
    }
    let mut seed = [0u8; 32];
    for (index, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| RuntimeError::Invalid("secret key is not valid hex".to_string()))?;
    }
    Ok(seed)
}
