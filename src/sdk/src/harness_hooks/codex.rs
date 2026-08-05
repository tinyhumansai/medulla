//! Codex hook delivery: the shared hook document as a `-c hooks=…` config
//! override.
//!
//! Verified against Codex 0.146.

use serde_json::Value;

use super::native::to_inline_toml;

/// The argv entries carrying `document` as Codex's `hooks` config override.
///
/// Codex parses `-c key=value` as TOML, and a command line cannot carry a
/// multi-line table, so the document is encoded as one inline table. Codex
/// merges config layers additively for hooks, so this adds to the operator's own
/// `~/.codex/hooks.json` rather than replacing it.
pub fn config_args(document: &Value) -> (Vec<String>, String) {
    let args = vec![
        "-c".to_string(),
        format!("hooks={}", to_inline_toml(document)),
    ];
    (args, TRUST_REQUIRED.to_string())
}

/// What the operator has to do before an injected Codex hook actually runs.
///
/// Codex records trust against a hook's content hash and *silently skips* hooks
/// it has not seen before — nothing on stderr, no non-zero exit. A per-spawn
/// injection is by construction absent from that store, so a Medulla hook does
/// nothing until it is trusted once.
///
/// Codex offers `--dangerously-bypass-hook-trust`, but it is invocation-wide: it
/// would also run any hook the checked-out repository declares in its own
/// `.codex/hooks.json`, turning a hook the operator configured into blanket
/// authorization for hooks they have never seen. Requiring one explicit trust is
/// the safe direction to fail — but only if it is said out loud, which is what
/// this message is for.
const TRUST_REQUIRED: &str = "Codex skips hooks it has not been told to trust, so Medulla's hooks \
     will not run until you trust them in Codex (`/hooks`). Medulla does not bypass that check: \
     the bypass is invocation-wide and would also authorize hooks shipped by the workspace.";
