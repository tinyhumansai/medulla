//! Tests for the clipboard module.
//!
//! The writer-dependent paths are driven through the `copy_with` /
//! `copy_for_operator_with` seams rather than through a platform name. Which
//! branch a platform selects depends on what the *test machine* has installed —
//! `pbcopy` on a developer's Mac, nothing in Linux CI — so a test written
//! against `writers("macos")` would silently exercise a different path in each
//! place and assert nothing reliable in either. The tmux hop is pinned the same
//! way: every test passes the context explicitly, so none of them depends on
//! whether the suite itself happens to be running inside tmux.

use super::tmux::{load_buffer_args, operator_sequence, osc52_from_params, passthrough};
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

/// A tmux context naming a socket no server is listening on, so `load_buffer`
/// fails the same way on a machine with tmux installed and one without.
fn dead_tmux() -> Tmux {
    Tmux::from_raw("/medulla-not-a-real-tmux-socket-xyz")
}

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
    let report = copy_with(BROKEN, None, "hello", |s| captured.push_str(s));
    assert_eq!(report.describe(), OSC_52);
    assert!(!report.confirmed());
    assert!(captured.contains("]52;c;"));
    assert!(captured.contains("aGVsbG8="), "base64(hello): {captured:?}");
}

#[test]
fn copy_uses_a_writer_when_one_succeeds_and_never_emits_the_escape() {
    // The ordinary local case: the text is on the real clipboard, so handing it
    // to the terminal as well would be redundant.
    let mut fired = false;
    let report = copy_with(WORKING, None, "x", |_| fired = true);
    assert_eq!(report.describe(), "cat");
    assert!(report.confirmed());
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
    let report = copy_with(mixed, None, "x", |_| fired = true);
    assert_eq!(report.writer.as_deref(), Some("cat"));
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
    let report = copy_for_operator_with(WORKING, None, "hello", |s| captured.push_str(s));
    assert_eq!(
        report.writer.as_deref(),
        Some("cat"),
        "the local writer also took it"
    );
    assert!(report.terminal, "the escape went out");
    assert!(captured.contains("]52;c;"), "{captured:?}");
    assert!(captured.contains("aGVsbG8="), "base64(hello): {captured:?}");
}

#[test]
fn the_operator_copy_reports_only_a_writer_it_confirmed() {
    // The terminal hand-off cannot be confirmed, so nothing is claimed.
    let mut fired = false;
    let report = copy_for_operator_with(BROKEN, None, "x", |_| fired = true);
    assert!(!report.confirmed());
    assert_eq!(report.describe(), OSC_52);
    assert!(fired, "the escape goes out regardless");
}

#[test]
fn a_dead_tmux_server_is_not_reported_as_a_copy() {
    // tmux may not be installed, and where it is the socket names no server:
    // either way the buffer did not take it, and the escape must still go out.
    let mut fired = false;
    let report = copy_with(BROKEN, Some(&dead_tmux()), "x", |_| fired = true);
    assert!(!report.tmux);
    assert!(
        fired,
        "nothing confirmed it, so the escape is the last hope"
    );
}

#[test]
fn copy_report_names_every_mechanism_that_took_it() {
    let report = CopyReport {
        writer: Some("pbcopy".to_string()),
        tmux: true,
        terminal: true,
    };
    assert_eq!(report.describe(), "tmux buffer + pbcopy + OSC 52");
    assert!(report.confirmed());
    assert_eq!(CopyReport::default().describe(), OSC_52);
}

#[test]
fn tmux_is_detected_from_the_env_var_and_its_socket_kept() {
    // `$TMUX` is untrusted, so `parse` only accepts a path naming a real
    // socket this process owns — a bound `UnixListener` stands in for the
    // server tmux itself would be running.
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("default");
    let _listener =
        std::os::unix::net::UnixListener::bind(&socket_path).expect("bind test socket");
    let socket = socket_path.to_str().expect("utf8 path").to_string();

    let tmux = Tmux::parse(Some(&format!("{socket},3423,0"))).expect("inside tmux");
    assert_eq!(tmux.socket(), socket);
    assert_eq!(
        Tmux::parse(Some(&socket)).map(|t| t.socket().to_string()),
        Some(socket.clone()),
        "a bare socket is accepted too"
    );

    assert_eq!(Tmux::parse(None), None);
    assert_eq!(Tmux::parse(Some("")), None, "an empty $TMUX is not tmux");
    assert_eq!(
        Tmux::parse(Some("relative/socket")),
        None,
        "a relative path is never a real tmux socket"
    );
    assert_eq!(
        Tmux::parse(Some("/medulla-not-a-real-tmux-socket-xyz")),
        None,
        "a socket that does not exist is rejected"
    );
    let regular_file = dir.path().join("not-a-socket");
    std::fs::write(&regular_file, b"not a socket").expect("write regular file");
    assert_eq!(
        Tmux::parse(Some(regular_file.to_str().expect("utf8 path"))),
        None,
        "a path that exists but is not a socket is rejected"
    );
}

#[test]
fn load_buffer_targets_our_own_server_and_asks_for_the_hand_off() {
    assert_eq!(
        load_buffer_args("/tmp/sock", true),
        vec!["-S", "/tmp/sock", "load-buffer", "-w", "-"]
    );
    // The fallback for tmux older than 3.2, where `-w` is not a valid flag.
    assert_eq!(
        load_buffer_args("/tmp/sock", false),
        vec!["-S", "/tmp/sock", "load-buffer", "-"]
    );
}

#[test]
fn passthrough_wraps_in_a_dcs_and_doubles_the_escapes() {
    let wrapped = passthrough("\x1b]52;c;aGk=\x07");
    assert_eq!(wrapped, "\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\");
}

#[test]
fn inside_tmux_the_escape_goes_out_plain_and_wrapped() {
    // Plain for a tmux that handles OSC 52 itself, wrapped for one that only
    // passes it through — neither is reliable alone, and both carry the same
    // text, so a terminal acting on both is not a problem.
    let inside = operator_sequence("hi", Some(&dead_tmux()));
    assert!(inside.starts_with("\x1b]52;c;aGk=\x07"), "{inside:?}");
    assert!(inside.contains("\x1bPtmux;"), "{inside:?}");
    assert!(inside.ends_with("\x1b\\"), "{inside:?}");

    // Outside tmux the wrapper buys nothing and is left off.
    assert_eq!(operator_sequence("hi", None), osc52("hi"));
}

#[test]
fn a_childs_clipboard_write_is_read_back_out_of_its_osc() {
    let params: &[&[u8]] = &[b"52", b"c", b"aGVsbG8="];
    assert_eq!(osc52_from_params(params).as_deref(), Some("hello"));
    // Unpadded base64: accepted, because emitters differ and terminals take both.
    let unpadded: &[&[u8]] = &[b"52", b"c", b"aGVsbG8"];
    assert_eq!(osc52_from_params(unpadded).as_deref(), Some("hello"));
}

#[test]
fn only_a_clipboard_write_is_forwarded() {
    // A query asks the terminal to report the clipboard back; passing it up as
    // a copy would put a literal "?" on the operator's clipboard.
    assert_eq!(osc52_from_params(&[b"52", b"c", b"?"]), None);
    // A clear would wipe whatever they had, having handed them nothing.
    assert_eq!(osc52_from_params(&[b"52", b"c", b""]), None);
    // Some other OSC — a window title, say.
    assert_eq!(osc52_from_params(&[b"0", b"a title"]), None);
    // Not base64, and base64 of bytes that are not text.
    assert_eq!(osc52_from_params(&[b"52", b"c", b"not base64!"]), None);
    assert_eq!(osc52_from_params(&[b"52", b"c", b"/w=="]), None);
    assert_eq!(osc52_from_params(&[]), None);
}

#[test]
fn current_platform_is_reported() {
    assert!(!current_platform().is_empty());
}
