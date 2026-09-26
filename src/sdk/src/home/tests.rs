//! Tests for the home module: the root/home split, the active-account marker,
//! and the `.env` loader.

use std::collections::HashMap;
use std::path::PathBuf;

use super::*;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn root_defaults_to_dot_medulla_under_home() {
    let root = medulla_root(&env(&[("HOME", "/home/dev")]));
    assert_eq!(root, PathBuf::from("/home/dev/.medulla"));
}

#[test]
fn dev_mode_uses_relative_dot_medulla() {
    assert_eq!(
        medulla_root(&env(&[("HOME", "/home/dev"), ("MEDULLA_DEV", "1")])),
        PathBuf::from(".medulla")
    );
    assert_eq!(
        medulla_root(&env(&[("MEDULLA_DEV", "TRUE")])),
        PathBuf::from(".medulla")
    );
    // A non-truthy value keeps the default.
    assert_eq!(
        medulla_root(&env(&[("HOME", "/home/dev"), ("MEDULLA_DEV", "no")])),
        PathBuf::from("/home/dev/.medulla")
    );
}

#[test]
fn explicit_home_beats_dev_and_default() {
    assert_eq!(
        medulla_root(&env(&[
            ("MEDULLA_HOME", "/custom/home"),
            ("MEDULLA_DEV", "1"),
            ("HOME", "/home/dev"),
        ])),
        PathBuf::from("/custom/home")
    );
    // An empty MEDULLA_HOME is ignored.
    assert_eq!(
        medulla_root(&env(&[("MEDULLA_HOME", ""), ("HOME", "/home/dev")])),
        PathBuf::from("/home/dev/.medulla")
    );
}

#[test]
fn userprofile_is_a_fallback_home() {
    assert_eq!(
        medulla_root(&env(&[("USERPROFILE", "C:/Users/dev")])),
        PathBuf::from("C:/Users/dev/.medulla")
    );
}

#[test]
fn a_signed_out_home_is_the_pre_login_account_directory() {
    // Not the root itself: the root holds account directories and the marker,
    // and nothing else, so a signed-out install still has a complete home.
    let tmp = tempfile::tempdir().expect("tempdir");
    let e = env(&[("MEDULLA_HOME", &tmp.path().to_string_lossy())]);
    assert_eq!(medulla_home(&e), tmp.path().join(user::PRE_LOGIN_USER_ID));
}

#[test]
fn the_marker_scopes_the_home_to_that_account() {
    let tmp = tempfile::tempdir().expect("tempdir");
    user::write_active_user_id(tmp.path(), "69d9cb73e61f755583c3671f").expect("write marker");
    let e = env(&[("MEDULLA_HOME", &tmp.path().to_string_lossy())]);
    assert_eq!(
        medulla_home(&e),
        tmp.path().join("69d9cb73e61f755583c3671f")
    );

    // The pre-login home is reachable again without signing in, which is what
    // `MEDULLA_USER=local` is for — nothing removes the marker, because logout
    // must leave the account findable.
    let back = env(&[
        ("MEDULLA_HOME", &tmp.path().to_string_lossy()),
        (user::MEDULLA_USER_ENV, user::PRE_LOGIN_USER_ID),
    ]);
    assert_eq!(
        medulla_home(&back),
        tmp.path().join(user::PRE_LOGIN_USER_ID)
    );
}

#[test]
fn switching_accounts_replaces_an_existing_marker() {
    // The write is a temp file plus a rename over a destination that already
    // exists. That is the shape a switch always takes, and it is the one
    // platforms disagree about — Windows `rename` replaces a file (std uses
    // `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`) but not a directory. This
    // runs on every platform CI covers, so the claim is checked rather than
    // assumed: a switch that silently kept naming the previous account would
    // leave the new one unreachable on the next launch.
    let tmp = tempfile::tempdir().expect("tempdir");
    let e = env(&[("MEDULLA_HOME", &tmp.path().to_string_lossy())]);

    user::write_active_user_id(tmp.path(), "first-account").expect("write the first marker");
    user::write_active_user_id(tmp.path(), "second-account").expect("replace it in place");

    assert_eq!(
        user::read_active_user_id(tmp.path()).as_deref(),
        Some("second-account")
    );
    assert_eq!(medulla_home(&e), tmp.path().join("second-account"));

    // And no temp file is left beside it — a rename that failed and fell back to
    // a copy would show up here.
    let strays: Vec<_> = std::fs::read_dir(tmp.path())
        .expect("read root")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name != user::ACTIVE_USER_FILE)
        .collect();
    assert!(strays.is_empty(), "left behind: {strays:?}");
}

#[test]
fn another_processs_login_does_not_re_home_a_running_one() {
    // The marker is shared mutable state. A daemon that re-read it mid-run would
    // start resolving account B's workflow store and log directory while the
    // core it booted — and the session it holds — stay account A's.
    let tmp = tempfile::tempdir().expect("tempdir");
    let e = env(&[("MEDULLA_HOME", &tmp.path().to_string_lossy())]);

    user::write_active_user_id(tmp.path(), "account-a").expect("write marker");
    assert_eq!(medulla_home(&e), tmp.path().join("account-a"));

    // Stand in for another process signing in: the file changes underneath us,
    // without going through this process's write path.
    std::fs::write(
        user::active_user_path(tmp.path()),
        "user_id = \"account-b\"\n",
    )
    .expect("rewrite the marker behind our back");

    assert_eq!(
        user::read_active_user_id(tmp.path()).as_deref(),
        Some("account-b"),
        "the file really did change"
    );
    assert_eq!(
        medulla_home(&e),
        tmp.path().join("account-a"),
        "a running process keeps the account it started as"
    );

    // This process changing it is the one thing that does move the pin — that is
    // what lets `medulla login` boot a core against the account it just wrote.
    user::write_active_user_id(tmp.path(), "account-c").expect("write marker");
    assert_eq!(medulla_home(&e), tmp.path().join("account-c"));
}

#[test]
fn the_env_override_beats_the_marker() {
    let tmp = tempfile::tempdir().expect("tempdir");
    user::write_active_user_id(tmp.path(), "written").expect("write marker");
    let e = env(&[
        ("MEDULLA_HOME", &tmp.path().to_string_lossy()),
        (user::MEDULLA_USER_ENV, "chosen"),
    ]);
    assert_eq!(medulla_home(&e), tmp.path().join("chosen"));
}

#[test]
fn an_id_that_could_escape_the_root_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for bad in ["../elsewhere", "a/b", ".hidden", "  ", "has space"] {
        assert!(
            user::write_active_user_id(tmp.path(), bad).is_err(),
            "{bad:?} should not be writable as an account id"
        );
        // And the same value arriving by env resolves to the pre-login home
        // rather than to a path outside the root.
        let e = env(&[
            ("MEDULLA_HOME", &tmp.path().to_string_lossy()),
            (user::MEDULLA_USER_ENV, bad),
        ]);
        assert_eq!(medulla_home(&e), tmp.path().join(user::PRE_LOGIN_USER_ID));
    }
}

#[test]
fn an_unreadable_marker_reads_as_signed_out() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(user::active_user_path(tmp.path()), "not toml at all {{").expect("write");
    assert_eq!(user::read_active_user_id(tmp.path()), None);

    std::fs::write(user::active_user_path(tmp.path()), "user_id = \"\"\n").expect("write");
    assert_eq!(user::read_active_user_id(tmp.path()), None);
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
