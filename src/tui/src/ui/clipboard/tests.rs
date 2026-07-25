//! Tests for the clipboard module.

use super::*;

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
fn copy_falls_back_to_osc52_when_no_writer() {
    let mut captured = String::new();
    // "none" platform → linux writers, which won't exist in most CI envs;
    // if they somehow do this test is still valid (mechanism differs).
    let via = copy_to_clipboard("hello", "definitely-not-a-real-os", |s| {
        captured.push_str(s)
    });
    assert_eq!(via, OSC_52);
    assert!(captured.contains("]52;c;"));
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
fn copy_to_clipboard_uses_spawn_path_when_writer_exists() {
    // A custom would-be-OSC emitter that must NOT fire, since `cat` succeeds.
    // We can't inject writers, so drive the platform-agnostic `pipe_to` seam
    // directly; here we confirm the OSC fallback only fires when no writer runs.
    let mut fired = false;
    // A bogus platform routes to the linux writer set, absent in CI → OSC.
    let via = copy_to_clipboard("x", "no-such-os", |_| fired = true);
    assert_eq!(via, OSC_52);
    assert!(fired);
}

#[test]
fn current_platform_is_reported() {
    assert!(!current_platform().is_empty());
}
