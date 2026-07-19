//! Pure URL/query helpers shared by the loopback flow, the CLI, and tests: build
//! the loopback redirect and backend login URLs, mint the state nonce,
//! percent-encode/decode query values, parse a request target, and summarize the
//! `/auth/me` response. No sockets or I/O — every function is a pure
//! transformation over its inputs.

use std::collections::HashMap;

use super::types::Provider;

/// The loopback redirect URI the backend sends the browser back to. The `state`
/// nonce is appended to the URI (`?state=<nonce>`) *before* it reaches the
/// backend, which preserves the loopback `redirectUri` verbatim and appends
/// `&token=`/`&error=` — so the callback query carries both the token and the
/// nonce we can validate against.
pub fn redirect_uri(port: u16, state: &str) -> String {
    format!("http://127.0.0.1:{port}/auth?state={state}")
}

/// Build the backend login URL for a provider, loopback port, and state nonce.
pub fn login_url(base_url: &str, provider: Provider, port: u16, state: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!(
        "{base}/auth/{}/login?redirect=app&redirectUri={}",
        provider.as_str(),
        percent_encode(&redirect_uri(port, state)),
    )
}

/// A random 32-hex-char (128-bit) state nonce derived from OS-seeded std
/// entropy — no `rand` dependency. `RandomState::new()` reseeds its SipHash keys
/// from the OS on every call, so the finished hashes vary across calls; we mix in
/// the process id, a monotonically-changing timestamp, and a stack address for
/// good measure.
pub fn random_state_nonce() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(std::process::id() as u64);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        h.write_u128(nanos);
        let stack_probe = 0u8;
        h.write_usize(&stack_probe as *const u8 as usize);
        let v = h.finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    hex_encode(&bytes)
}

/// Lowercase hex-encode a byte slice.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Summarize an `/auth/me` response for the "who am I" line.
pub fn describe_me(me: &serde_json::Value) -> String {
    let obj = me.get("user").unwrap_or(me);
    let email = obj.get("email").and_then(|v| v.as_str());
    let id = obj
        .get("id")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("userId").and_then(|v| v.as_str()));
    match (email, id) {
        (Some(e), Some(i)) => format!("Logged in as {e} ({i})"),
        (Some(e), None) => format!("Logged in as {e}"),
        (None, Some(i)) => format!("Logged in as {i}"),
        (None, None) => "Logged in.".to_string(),
    }
}

/// Percent-encode a string, escaping everything outside the unreserved set.
pub(super) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-decode a query value (`%XX` and `+` → space).
pub(super) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Split a request target into `(path, query)`, percent-decoding query values.
pub(super) fn parse_target(target: &str) -> (String, HashMap<String, String>) {
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let params = query
        .split('&')
        .filter_map(|pair| {
            if pair.is_empty() {
                return None;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if k.is_empty() {
                return None;
            }
            Some((percent_decode(k), percent_decode(v)))
        })
        .collect();
    (path.to_string(), params)
}
