//! The Medulla home directory and the early `.env` loader.
//!
//! Everything Medulla persists — credentials, TUI state, the tiny.place
//! identity, and the layered config file — lives under a single home directory
//! resolved by [`medulla_home`]. The resolver is pure over an injected env map
//! so it can be unit-tested without touching the real process environment;
//! `main` wires the real environment in.

use std::collections::HashMap;
use std::path::PathBuf;

/// Whether an env value is truthy: `"1"` or `"true"` (case-insensitive, trimmed).
pub fn is_truthy(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true")
}

/// Resolve the Medulla home directory (where all config/data lives).
///
/// Precedence:
/// 1. `MEDULLA_HOME` — an explicit path wins over everything.
/// 2. `MEDULLA_DEV` truthy — a local-dev home at `./.medulla` (relative to cwd).
/// 3. otherwise `<home>/.medulla`, where `<home>` comes from `HOME` /
///    `USERPROFILE` (or [`dirs::home_dir`] as a last resort).
pub fn medulla_home(env: &HashMap<String, String>) -> PathBuf {
    if let Some(explicit) = env
        .get("MEDULLA_HOME")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        return PathBuf::from(explicit);
    }
    if env
        .get("MEDULLA_DEV")
        .map(|v| is_truthy(v))
        .unwrap_or(false)
    {
        return PathBuf::from(".medulla");
    }
    home_base(env).join(".medulla")
}

/// The user's OS home directory, from the injected env first (`HOME`, then
/// `USERPROFILE`), falling back to [`dirs::home_dir`] and finally `.`.
fn home_base(env: &HashMap<String, String>) -> PathBuf {
    if let Some(h) = env.get("HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(h);
    }
    if let Some(h) = env.get("USERPROFILE").filter(|s| !s.is_empty()) {
        return PathBuf::from(h);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Parse a `.env` file body into ordered `KEY=VALUE` pairs.
///
/// Recognizes: `#` comment lines, an optional `export ` prefix, and single- or
/// double-quoted values (the quotes are stripped). Blank lines and lines without
/// an `=` are skipped. This is intentionally minimal — no interpolation, no
/// multi-line values.
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

#[cfg(test)]
#[path = "home_tests.rs"]
mod tests;
