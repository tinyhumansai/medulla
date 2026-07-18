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
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn home_defaults_to_dot_medulla_under_home() {
        let home = medulla_home(&env(&[("HOME", "/home/dev")]));
        assert_eq!(home, PathBuf::from("/home/dev/.medulla"));
    }

    #[test]
    fn dev_mode_uses_relative_dot_medulla() {
        assert_eq!(
            medulla_home(&env(&[("HOME", "/home/dev"), ("MEDULLA_DEV", "1")])),
            PathBuf::from(".medulla")
        );
        assert_eq!(
            medulla_home(&env(&[("MEDULLA_DEV", "TRUE")])),
            PathBuf::from(".medulla")
        );
        // A non-truthy value keeps the default.
        assert_eq!(
            medulla_home(&env(&[("HOME", "/home/dev"), ("MEDULLA_DEV", "no")])),
            PathBuf::from("/home/dev/.medulla")
        );
    }

    #[test]
    fn explicit_home_beats_dev_and_default() {
        assert_eq!(
            medulla_home(&env(&[
                ("MEDULLA_HOME", "/custom/home"),
                ("MEDULLA_DEV", "1"),
                ("HOME", "/home/dev"),
            ])),
            PathBuf::from("/custom/home")
        );
        // An empty MEDULLA_HOME is ignored.
        assert_eq!(
            medulla_home(&env(&[("MEDULLA_HOME", ""), ("HOME", "/home/dev")])),
            PathBuf::from("/home/dev/.medulla")
        );
    }

    #[test]
    fn userprofile_is_a_fallback_home() {
        assert_eq!(
            medulla_home(&env(&[("USERPROFILE", "C:/Users/dev")])),
            PathBuf::from("C:/Users/dev/.medulla")
        );
    }

    #[test]
    fn dotenv_parses_comments_quotes_and_export() {
        let body = "\
# a comment\n\
\n\
FOO=bar\n\
export BAZ=qux\n\
QUOTED=\"hello world\"\n\
SINGLE='tick'\n\
  SPACED = spaced-value \n\
EMPTY=\n\
noeq line\n\
=novalue\n";
        let pairs = parse_dotenv(body);
        assert_eq!(
            pairs,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
                ("QUOTED".to_string(), "hello world".to_string()),
                ("SINGLE".to_string(), "tick".to_string()),
                ("SPACED".to_string(), "spaced-value".to_string()),
                ("EMPTY".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn apply_dotenv_never_overrides_existing() {
        let mut e = env(&[("FOO", "already")]);
        apply_dotenv(
            &mut e,
            vec![
                ("FOO".to_string(), "new".to_string()),
                ("BAR".to_string(), "fresh".to_string()),
            ],
        );
        assert_eq!(e.get("FOO").map(String::as_str), Some("already"));
        assert_eq!(e.get("BAR").map(String::as_str), Some("fresh"));
    }

    #[test]
    fn is_truthy_matches_one_and_true() {
        assert!(is_truthy("1"));
        assert!(is_truthy(" TRUE "));
        assert!(!is_truthy("0"));
        assert!(!is_truthy("yes"));
    }
}
