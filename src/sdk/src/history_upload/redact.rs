//! Client-side secret scrubbing, applied before any transcript leaves the
//! machine.
//!
//! Two rules govern every pattern here:
//!
//! 1. **Never break the JSON.** Transcripts are JSONL and the backend parses
//!    them to score the user. Replacements stay inside string values —
//!    `[REDACTED]` needs no escaping, and every value pattern excludes `"` so a
//!    match cannot span a string boundary.
//! 2. **Never eat scoring metadata.** Token counters, timestamps, roles, and
//!    tool names must survive verbatim, or the user is under-credited for work
//!    they really did. The generic `key = value` rule therefore skips
//!    all-numeric values, which is what every `*_tokens` field holds.
//!
//! Patterns are deliberately conservative: a missed exotic secret is a smaller
//! harm than a corrupted transcript or a silently zeroed score. High-entropy
//! heuristics (bare hex or base64 blobs) are omitted on purpose — 40-char hex
//! matches every git SHA, which transcripts are full of and which is not secret.

use std::sync::OnceLock;

use regex::Regex;

/// The placeholder every scrubbed secret is replaced with.
pub const REDACTED: &str = "[REDACTED]";

/// Patterns whose entire match is a secret and is replaced wholesale.
fn whole_match_rules() -> &'static [Regex] {
    static RULES: OnceLock<Vec<Regex>> = OnceLock::new();
    RULES.get_or_init(|| {
        [
            // PEM private key blocks. `[^"]` keeps the match inside one JSON
            // string (transcripts escape newlines, so the block is on one line).
            r#"-----BEGIN[^"]{0,64}PRIVATE KEY-----[^"]*?-----END[^"]{0,64}PRIVATE KEY-----"#,
            // OpenAI / Anthropic style keys (`sk-`, `sk-ant-`, `sk-proj-`).
            r"sk-[A-Za-z0-9_\-]{16,}",
            // AWS access key id.
            r"AKIA[0-9A-Z]{16}",
            // GitHub tokens (ghp_, gho_, ghu_, ghs_, ghr_).
            r"gh[pousr]_[A-Za-z0-9]{20,}",
            // Slack tokens.
            r"xox[baprs]-[A-Za-z0-9\-]{10,}",
            // Google API keys.
            r"AIza[0-9A-Za-z_\-]{35}",
            // JSON Web Tokens.
            r"eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).expect("redaction pattern must compile"))
        .collect()
    })
}

/// `Bearer <token>` — the scheme is kept so the shape of the transcript reads
/// naturally; only the credential is replaced.
fn bearer_rule() -> &'static Regex {
    static RULE: OnceLock<Regex> = OnceLock::new();
    RULE.get_or_init(|| {
        Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=\-]{20,}")
            .expect("redaction pattern must compile")
    })
}

/// Secret-looking assignments: `api_key: "..."`, `PASSWORD=...`, `token=...`.
///
/// A bare `token` key is included — `"token":"…"` is one of the commonest ways a
/// credential ends up pasted into a session, and the consent screen promises
/// tokens are stripped. Protecting the scorer's counters is handled by
/// [`is_token_counter_key`] instead, which is precise about the plural
/// `*_tokens` family rather than excluding the whole word.
fn assignment_rule() -> &'static Regex {
    static RULE: OnceLock<Regex> = OnceLock::new();
    RULE.get_or_init(|| {
        Regex::new(
            r#"(?i)([A-Za-z0-9_\-]*(?:api[_\-]?key|secret|password|passwd|credential|private[_\-]?key|token)[A-Za-z0-9_\-]*"?\s*[:=]\s*"?)([^\s"',}\]{]{8,})"#,
        )
        .expect("redaction pattern must compile")
    })
}

/// Whether a matched assignment key is one of the scorer's token counters.
///
/// Every counter the backend reads is plural — `input_tokens`, `output_tokens`,
/// `total_tokens`, `cache_read_input_tokens` — while credentials are singular
/// (`token`, `access_token`, `api_token`). Keying off that distinction lets bare
/// `token` assignments be redacted without ever touching a usage number, which
/// would silently under-credit the user.
fn is_token_counter_key(prefix: &str) -> bool {
    let key: String = prefix
        .trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ':' | '='))
        .trim_matches(|ch: char| ch == '"' || ch == '\'')
        .to_ascii_lowercase();
    key.ends_with("tokens")
}

/// Scrubs secrets from `input`, returning the cleaned text and how many
/// replacements were made.
///
/// Structure-preserving: token counters, timestamps, and tool names pass through
/// untouched, so the backend can still score the transcript.
pub fn redact_text(input: &str) -> (String, usize) {
    let mut text = input.to_string();
    let mut redactions = 0usize;

    for rule in whole_match_rules() {
        let matches = rule.find_iter(&text).count();
        if matches > 0 {
            redactions += matches;
            text = rule.replace_all(&text, REDACTED).into_owned();
        }
    }

    let bearer = bearer_rule();
    let bearer_matches = bearer.find_iter(&text).count();
    if bearer_matches > 0 {
        redactions += bearer_matches;
        text = bearer
            .replace_all(&text, format!("${{1}}{REDACTED}").as_str())
            .into_owned();
    }

    // The assignment rule needs a guard rather than a plain replace, but the
    // guard is on the *key*, not the value: `"total_tokens": 12345678` must
    // survive, while a numeric secret (`password: 12345678`, a PIN, a recovery
    // code) must not. Judging by value shape would leak exactly those.
    let assignment = assignment_rule();
    let mut assignment_hits = 0usize;
    let replaced = assignment.replace_all(&text, |caps: &regex::Captures<'_>| {
        let prefix = &caps[1];
        let value = &caps[2];
        if is_token_counter_key(prefix) {
            return format!("{prefix}{value}");
        }
        assignment_hits += 1;
        format!("{prefix}{REDACTED}")
    });
    text = replaced.into_owned();
    redactions += assignment_hits;

    (text, redactions)
}
