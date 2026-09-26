//! Parsing and applying a `.env` file, loaded before anything reads the
//! environment.
//!
//! Deliberately minimal — no interpolation, no multi-line values — because this
//! runs at the very start of `main`, ahead of the home resolution every other
//! module depends on, and a surprising parse there is a surprising home.

use std::collections::HashMap;

/// Parse a `.env` file body into ordered `KEY=VALUE` pairs.
///
/// Recognizes: `#` comment lines, an optional `export ` prefix, and single- or
/// double-quoted values (the quotes are stripped). Blank lines and lines without
/// an `=` are skipped.
pub fn parse_dotenv(contents: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line
            .strip_prefix("export ")
            .map(str::trim_start)
            .unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        out.push((key.to_string(), strip_quotes(value.trim())));
    }
    out
}

/// Strip a single matching pair of surrounding single or double quotes.
fn strip_quotes(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// Apply parsed `.env` pairs into `env`, never overriding a key already present.
/// Used both by `main` (over the real process env, via a wrapper) and by tests.
pub fn apply_dotenv(env: &mut HashMap<String, String>, pairs: Vec<(String, String)>) {
    for (key, value) in pairs {
        env.entry(key).or_insert(value);
    }
}

/// Load a `.env` file from the current directory into the real process
/// environment (if present), never overriding variables already set. Best-effort:
/// a missing or unreadable file is silently ignored. Called very early in `main`.
pub fn load_dotenv_from_cwd() {
    let contents = match std::fs::read_to_string(".env") {
        Ok(text) => text,
        Err(_) => return,
    };
    for (key, value) in parse_dotenv(&contents) {
        if std::env::var_os(&key).is_none() {
            std::env::set_var(&key, &value);
        }
    }
}
