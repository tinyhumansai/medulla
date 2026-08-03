//! Tests for the entry module.

use super::*;
// `Flags`/`parse_provider` come from `super::*`; the provider enum is only
// needed by these tests, so import it explicitly here.
use crate::protocol::HarnessProvider;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parses_values_bools_and_lists() {
    let flags = Flags::parse(&args(&[
        "--workspace",
        "/tmp/x",
        "--providers",
        "claude,codex",
        "--providers",
        "opencode",
        "--once",
        "--dangerously-skip-permissions",
    ]))
    .unwrap();
    assert_eq!(flags.string("workspace").as_deref(), Some("/tmp/x"));
    assert!(flags.is_set("once"));
    assert!(flags.is_set("dangerously-skip-permissions"));
    assert!(!flags.is_set("no-onboard"));
    // Repeated + comma-joined lists flatten and trim.
    assert_eq!(
        flags.list("providers").unwrap(),
        vec!["claude", "codex", "opencode"]
    );
    // A later value wins for scalar lookups.
    let dup = Flags::parse(&args(&["--model", "a", "--model", "b"])).unwrap();
    assert_eq!(dup.string("model").as_deref(), Some("b"));
}

#[test]
fn rejects_unknown_and_valueless_flags() {
    assert!(Flags::parse(&args(&["positional"])).is_err());
    match Flags::parse(&args(&["--model"])) {
        Err(err) => assert!(err.contains("needs a value"), "got: {err}"),
        Ok(_) => panic!("missing value should error"),
    }
}

#[test]
fn number_and_positive_validation() {
    let flags = Flags::parse(&args(&[
        "--concurrency",
        "3",
        "--zero",
        "0",
        "--bad",
        "nope",
    ]))
    .unwrap();
    assert_eq!(flags.number("concurrency").unwrap(), Some(3));
    assert_eq!(flags.number("missing").unwrap(), None);
    assert!(flags.number("bad").is_err());
    assert_eq!(flags.positive("concurrency", 2).unwrap(), 3);
    assert_eq!(flags.positive("missing", 7).unwrap(), 7);
    assert!(flags.positive("zero", 2).is_err());
}

#[test]
fn parse_provider_maps_wire_names() {
    assert_eq!(parse_provider("claude").unwrap(), HarnessProvider::Claude);
    assert_eq!(parse_provider("codex").unwrap(), HarnessProvider::Codex);
    let err = parse_provider("bogus").unwrap_err();
    assert!(err.contains("unknown provider"), "got: {err}");
}
