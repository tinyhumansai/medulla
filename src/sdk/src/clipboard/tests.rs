//! Tests for the clipboard module.
//!
//! The writer-dependent paths are driven through the `copy_with` /
//! `copy_for_operator_with` seams rather than through a platform name. Which
//! branch a platform selects depends on what the *test machine* has installed —
//! `pbcopy` on a developer's Mac, nothing in Linux CI — so a test written
//! against `writers("macos")` would silently exercise a different path in each
//! place and assert nothing reliable in either.

use super::*;

/// A writer that always succeeds: `cat` drains stdin and exits 0.
const WORKING: &[Writer] = &[Writer {
    cmd: "cat",
    args: &[],
}];

/// A writer that is never installed, so every attempt fails.
const BROKEN: &[Writer] = &[Writer {
    cmd: "medulla-not-a-real-binary-xyz",
    args: &[],
}];

#[test]
fn osc52_wraps_base64() {
    let s = osc52("hi");
    assert!(s.starts_with("\x1b]52;c;"));
    assert!(s.ends_with('\x07'));
    assert!(s.contains("aGk=")); // base64("hi")
}

#[test]
fn writers_by_platform() {
    assert_eq!(writers("macos")[0].cmd, "pbcopy");
    assert_eq!(writers("windows")[0].cmd, "clip");
    assert_eq!(writers("linux")[0].cmd, "wl-copy");
    assert_eq!(writers("freebsd")[0].cmd, "wl-copy");
}

#[test]
fn copy_falls_back_to_osc52_when_no_writer_takes_it() {
    let mut captured = String::new();
    let via = copy_with(BROKEN, "hello", |s| captured.push_str(s));
    assert_eq!(via, OSC_52);
    assert!(captured.contains("]52;c;"));
    assert!(captured.contains("aGVsbG8="), "base64(hello): {captured:?}");
}

#[test]
fn copy_uses_a_writer_when_one_succeeds_and_never_emits_the_escape() {
    // The ordinary local case: the text is on the real clipboard, so handing it
    // to the terminal as well would be redundant.
    let mut fired = false;
    let via = copy_with(WORKING, "x", |_| fired = true);
    assert_eq!(via, "cat");
    assert!(
        !fired,
        "the OSC fallback must not fire once a writer took it"
    );
}

#[test]
fn copy_falls_through_a_failing_writer_to_a_working_one() {
    let mixed: &[Writer] = &[
        Writer {
            cmd: "medulla-not-a-real-binary-xyz",
            args: &[],
        },
        Writer {
            cmd: "cat",
            args: &[],
        },
    ];
    let mut fired = false;
    assert_eq!(copy_with(mixed, "x", |_| fired = true), "cat");
    assert!(!fired);
}

#[test]
fn pipe_to_missing_binary_is_false() {
    assert!(!pipe_to("medulla-not-a-real-binary-xyz", &[], "hi"));
}

#[test]
fn pipe_to_succeeds_for_a_stdin_reader() {
    // `cat` drains stdin and exits 0 — the spawn/pipe success path.
    assert!(pipe_to("cat", &[], "clipboard payload"));
}

#[test]
fn pipe_to_is_false_when_the_child_exits_non_zero() {
    // `false` reads nothing and exits 1. Reporting it as a completed copy would
    // stop the caller trying the next writer.
    assert!(!pipe_to("false", &[], "hi"));
}

#[test]
fn the_operator_copy_always_reaches_the_terminal_first() {
    // The whole point: on a box reached over SSH, a local writer succeeding must
    // not stop the escape that reaches the operator's actual machine.
    let mut captured = String::new();
    let via = copy_for_operator_with(WORKING, "hello", |s| captured.push_str(s));
    assert_eq!(via.as_deref(), Some("cat"), "the local writer also took it");
    assert!(captured.contains("]52;c;"), "{captured:?}");
    assert!(captured.contains("aGVsbG8="), "base64(hello): {captured:?}");
}

#[test]
fn the_operator_copy_reports_only_a_writer_it_confirmed() {
    // The terminal hand-off cannot be confirmed, so nothing is claimed.
    let mut fired = false;
    assert_eq!(copy_for_operator_with(BROKEN, "x", |_| fired = true), None);
    assert!(fired, "the escape goes out regardless");
}

#[test]
fn current_platform_is_reported() {
    assert!(!current_platform().is_empty());
}
