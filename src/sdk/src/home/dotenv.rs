//! Parsing and applying a `.env` file, loaded before anything reads the
//! environment.
//!
//! Deliberately minimal — no interpolation, no multi-line values — because this
//! runs at the very start of `main`, ahead of the home resolution every other
//! module depends on, and a surprising parse there is a surprising home.
//!
//! A `.env` belongs to whatever repository Medulla was opened in, so it is
//! untrusted input. It may configure the run, but it may not choose *where
//! Medulla's home is* or *which account inside it is used*: the variables in
//! [`HOME_SELECTORS`] are refused from it. Otherwise a repository could ship a
//! `.env` (and, for `MEDULLA_DEV`, a `./.medulla` directory beside it) that
//! points Medulla at a planted session, config, or credential store. Those
//! variables still work from the invoking shell.

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

/// Variables that select Medulla's home or the account inside it, which a cwd
/// `.env` may not set (see the module docs). Each one moves where sessions,
/// credentials, and config are read from: the root itself (`MEDULLA_HOME`,
/// `MEDULLA_DEV`'s `./.medulla`, the OS home `HOME`/`USERPROFILE` it defaults
/// under), the account within it (`MEDULLA_USER`), or the config file that can
/// carry a credential (`MEDULLA_CONFIG_PATH`).
pub const HOME_SELECTORS: &[&str] = &[
    "MEDULLA_HOME",
    "MEDULLA_DEV",
    super::user::MEDULLA_USER_ENV,
    crate::config::CONFIG_PATH_ENV,
    "HOME",
    "USERPROFILE",
];

/// Whether `key` is one a `.env` may not set.
fn is_home_selector(key: &str) -> bool {
    HOME_SELECTORS.contains(&key)
}

/// Apply parsed `.env` pairs into `env`, never overriding a key already present
/// and never setting a [`HOME_SELECTORS`] key. Returns the home-selector keys
/// that were refused, in file order, for the caller to report.
///
/// The pure counterpart of [`load_dotenv_from_cwd`], over an injected map.
pub fn apply_dotenv(
    env: &mut HashMap<String, String>,
    pairs: Vec<(String, String)>,
) -> Vec<String> {
    let mut refused = Vec::new();
    for (key, value) in pairs {
        if is_home_selector(&key) {
            refused.push(key);
            continue;
        }
        env.entry(key).or_insert(value);
    }
    refused
}

/// Load a `.env` file from the current directory into the real process
/// environment (if present), never overriding variables already set and never
/// setting a [`HOME_SELECTORS`] key. Best-effort: a missing or unreadable file
/// is silently ignored. Called very early in `main`.
///
/// Returns the home-selector keys the file tried to set, so `main` can say
/// they were ignored rather than leave the operator wondering why a `.env`
/// `MEDULLA_DEV=1` had no effect.
pub fn load_dotenv_from_cwd() -> Vec<String> {
    let contents = match std::fs::read_to_string(".env") {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };
    let mut refused = Vec::new();
    for (key, value) in parse_dotenv(&contents) {
        if is_home_selector(&key) {
            refused.push(key);
            continue;
        }
        if std::env::var_os(&key).is_none() {
            std::env::set_var(&key, &value);
        }
    }
    refused
}
