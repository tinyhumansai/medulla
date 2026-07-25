//! Tests for the home module.

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
