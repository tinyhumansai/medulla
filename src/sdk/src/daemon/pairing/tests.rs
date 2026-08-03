//! Tests for the pairing hand-off.

use super::*;

#[test]
fn the_address_is_handed_to_the_terminal_only_when_there_is_one() {
    let escape = clipboard_handoff("abc123", true).expect("a terminal takes the escape");
    // OSC 52, clipboard selection, base64 of the address.
    assert!(escape.starts_with("\x1b]52;c;"), "{escape:?}");
    assert!(escape.contains("YWJjMTIz"), "{escape:?}");
    // Under systemd or in a pipeline the escape would land in a log file.
    assert!(clipboard_handoff("abc123", false).is_none());
}

#[test]
fn an_empty_address_is_never_handed_over() {
    // Onboarding can be skipped (`--no-onboard`), and copying an empty string
    // would silently clear whatever the operator had on their clipboard.
    assert!(clipboard_handoff("", true).is_none());
    assert!(clipboard_handoff("   ", true).is_none());
}

#[test]
fn the_banner_puts_the_address_on_a_line_of_its_own() {
    let banner = pairing_banner("abc123", None, false);
    assert!(
        banner.lines().any(|l| l.trim() == "abc123"),
        "the address must be selectable on its own line: {banner}"
    );
    assert!(banner.contains("Routing › Add Host"), "{banner}");
}

#[test]
fn the_banner_only_claims_a_clipboard_it_reached() {
    assert!(!pairing_banner("abc123", None, false).contains("clipboard"));
    assert!(pairing_banner("abc123", None, true).contains("clipboard"));
}

#[test]
fn a_registered_handle_is_offered_as_the_shorter_thing_to_type() {
    let banner = pairing_banner("abc123", Some("build-box"), true);
    assert!(banner.contains("@build-box"), "{banner}");
    // One `@`, whether or not the caller already prefixed it.
    let prefixed = pairing_banner("abc123", Some("@build-box"), true);
    assert!(
        prefixed.contains("@build-box") && !prefixed.contains("@@"),
        "{prefixed}"
    );
}

#[test]
fn a_blank_handle_is_treated_as_no_handle() {
    let banner = pairing_banner("abc123", Some("  "), false);
    assert!(!banner.contains('@'), "{banner}");
}

#[test]
fn the_remote_join_command_is_one_pasteable_line() {
    // A multi-line command would paste as several shell invocations, and the
    // continuation escapes in the literal are easy to get wrong.
    assert!(!REMOTE_JOIN_COMMAND.contains('\n'), "{REMOTE_JOIN_COMMAND}");
    assert!(!REMOTE_JOIN_COMMAND.contains("  "), "{REMOTE_JOIN_COMMAND}");
    // Installation is guarded. Starting the daemon is intentionally not part
    // of this line: a fresh host still needs a provisioned link identity.
    assert!(REMOTE_JOIN_COMMAND.starts_with("command -v medulla"));
    assert!(!REMOTE_JOIN_COMMAND.contains("medulla daemon"));
}
