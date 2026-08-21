//! Unit tests for the embedded harness pane, split so no file exceeds the
//! repo's 500-line ceiling: this module covers key encoding, mouse-wheel
//! encoding, and focus; [`buttons`] covers click/drag/release encoding and the
//! per-mode gate on it; [`session`] drives a real child on a real
//! pseudo-terminal to cover the session-facing half of [`super::LocalSessions`];
//! [`spawn`] covers what a session is opened *with* — provider choice,
//! attribution, and the argv a real spawn produces; [`shells`] covers the plain
//! `bash`/`zsh` door beside them.
//!
//! The encoder is where a mistake is invisible until an operator is sitting in
//! front of a harness that ignores their arrow keys, so every family it emits is
//! pinned to a literal byte sequence rather than to a round-trip through
//! crossterm — a round-trip would pass just as happily if both directions were
//! wrong together.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// Unix-only: drives a real child on a real pseudo-terminal via `/bin/sh`,
// which Windows has no equivalent of. The pane layer itself is portable; this
// half of its tests is not.
#[cfg(unix)]
mod session;

// Same constraint as [`session`]: it opens real sessions to check what each
// creation door stamps on them.
#[cfg(unix)]
mod origin;

// Same constraint as [`session`]: it spawns real children (or stand-ins) to
// check what a real spawn produces, not just the argv builder in isolation.
#[cfg(unix)]
mod spawn;

// Same constraint as [`session`]: it starts a real shell on a real pty to
// check what that door hands the child — and, mostly, what it does not.
#[cfg(unix)]
mod shells;

mod buttons;

use super::keys::{encode, is_focus_chord};
use super::mouse;
use super::HarnessFocus;

/// Decode a raw input byte the way crossterm does, so chord tests assert against
/// what a terminal actually delivers rather than against a key event we invented.
///
/// This mirrors `crossterm::event::sys::unix::parse`: `0x01..=0x1A` count up from
/// `'a'`, and the `0x1C..=0x1F` block counts up from `'4'`. That second rule is
/// the whole reason this helper exists — it means `Ctrl-]` (`0x1D`) arrives as
/// `Ctrl+'5'`, and a chord matched on `Ctrl+']'` never fires on a real terminal.
fn as_terminal_sends(byte: u8) -> KeyEvent {
    let code = match byte {
        0x01..=0x1A => KeyCode::Char((byte - 0x1 + b'a') as char),
        0x1C..=0x1F => KeyCode::Char((byte - 0x1C + b'4') as char),
        other => KeyCode::Char(other as char),
    };
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

/// Encode a key with no modifiers.
fn plain(code: KeyCode) -> Vec<u8> {
    encode(KeyEvent::new(code, KeyModifiers::NONE)).expect("encodable")
}

/// Encode a key with modifiers.
fn with(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
    encode(KeyEvent::new(code, mods)).expect("encodable")
}

#[test]
fn printable_characters_pass_through_as_utf8() {
    assert_eq!(plain(KeyCode::Char('a')), b"a");
    assert_eq!(plain(KeyCode::Char('Z')), b"Z");
    // Multi-byte characters are not truncated to one byte.
    assert_eq!(plain(KeyCode::Char('é')), "é".as_bytes());
}

#[test]
fn enter_is_carriage_return_not_newline() {
    // `\n` types a literal newline into a harness composer instead of
    // submitting, which is the single most confusing way for this to be wrong.
    assert_eq!(plain(KeyCode::Enter), b"\r");
}

#[test]
fn backspace_is_del_not_backspace_byte() {
    // `\x08` would read as Ctrl-H inside the harness.
    assert_eq!(plain(KeyCode::Backspace), b"\x7f");
}

#[test]
fn control_letters_use_the_c0_range() {
    assert_eq!(with(KeyCode::Char('a'), KeyModifiers::CONTROL), vec![1]);
    assert_eq!(with(KeyCode::Char('c'), KeyModifiers::CONTROL), vec![3]);
    assert_eq!(with(KeyCode::Char('z'), KeyModifiers::CONTROL), vec![26]);
    // Case-insensitive: a shifted Ctrl-C is still ETX.
    assert_eq!(with(KeyCode::Char('C'), KeyModifiers::CONTROL), vec![3]);
}

#[test]
fn control_symbols_continue_the_c0_run() {
    assert_eq!(with(KeyCode::Char('['), KeyModifiers::CONTROL), vec![0x1b]);
    assert_eq!(with(KeyCode::Char(']'), KeyModifiers::CONTROL), vec![0x1d]);
    assert_eq!(with(KeyCode::Char(' '), KeyModifiers::CONTROL), vec![0]);
}

#[test]
fn unmodified_cursor_keys_are_bare_csi() {
    assert_eq!(plain(KeyCode::Up), b"\x1b[A");
    assert_eq!(plain(KeyCode::Down), b"\x1b[B");
    assert_eq!(plain(KeyCode::Right), b"\x1b[C");
    assert_eq!(plain(KeyCode::Left), b"\x1b[D");
    assert_eq!(plain(KeyCode::Home), b"\x1b[H");
    assert_eq!(plain(KeyCode::End), b"\x1b[F");
}

#[test]
fn modified_cursor_keys_carry_the_xterm_parameter() {
    // 1 + shift + 2*alt + 4*ctrl, then +1.
    assert_eq!(with(KeyCode::Up, KeyModifiers::SHIFT), b"\x1b[1;2A");
    assert_eq!(with(KeyCode::Left, KeyModifiers::ALT), b"\x1b[1;3D");
    assert_eq!(with(KeyCode::Right, KeyModifiers::CONTROL), b"\x1b[1;5C");
    assert_eq!(
        with(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        b"\x1b[1;6B"
    );
}

#[test]
fn tilde_keys_encode_plain_and_modified() {
    assert_eq!(plain(KeyCode::Delete), b"\x1b[3~");
    assert_eq!(plain(KeyCode::PageUp), b"\x1b[5~");
    assert_eq!(with(KeyCode::Delete, KeyModifiers::CONTROL), b"\x1b[3;5~");
}

#[test]
fn function_keys_split_between_ss3_and_csi() {
    assert_eq!(plain(KeyCode::F(1)), b"\x1bOP");
    assert_eq!(plain(KeyCode::F(4)), b"\x1bOS");
    assert_eq!(plain(KeyCode::F(5)), b"\x1b[15~");
    // The VT220 gaps: F6 is 17, not 16, and F11 is 23, not 22.
    assert_eq!(plain(KeyCode::F(6)), b"\x1b[17~");
    assert_eq!(plain(KeyCode::F(10)), b"\x1b[21~");
    assert_eq!(plain(KeyCode::F(11)), b"\x1b[23~");
    assert_eq!(plain(KeyCode::F(12)), b"\x1b[24~");
}

#[test]
fn shift_tab_is_its_own_sequence() {
    assert_eq!(plain(KeyCode::BackTab), b"\x1b[Z");
}

#[test]
fn alt_prefixes_a_plain_key_with_escape() {
    assert_eq!(with(KeyCode::Char('b'), KeyModifiers::ALT), b"\x1bb");
    assert_eq!(with(KeyCode::Enter, KeyModifiers::ALT), b"\x1b\r");
}

#[test]
fn alt_does_not_double_escape_a_csi_sequence() {
    // The modifier is already in the parameter; an extra ESC would arrive as a
    // stray Escape followed by a sequence the harness cannot parse.
    let bytes = with(KeyCode::Up, KeyModifiers::ALT);
    assert_eq!(bytes, b"\x1b[1;3A");
    assert!(!bytes.starts_with(b"\x1b\x1b"));
}

#[test]
fn keys_with_no_terminal_form_encode_to_nothing() {
    // A modifier press on its own is not a keystroke a terminal transmits.
    assert!(encode(KeyEvent::new(
        KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftControl),
        KeyModifiers::NONE
    ))
    .is_none());
}

#[test]
fn the_focus_chord_is_recognised_as_the_terminal_actually_sends_it() {
    // The regression this pins: `Ctrl-]` is the byte 0x1D, which crossterm
    // decodes to `Ctrl+'5'`. Matching only `Ctrl+']'` compiles, passes a
    // synthetic round-trip test, and then does nothing at all when a human
    // presses the key.
    assert!(is_focus_chord(as_terminal_sends(0x1d)));
    // The kitty keyboard protocol reports the real key instead, so both
    // spellings have to work.
    assert!(is_focus_chord(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL
    )));
}

#[test]
fn ordinary_keys_are_not_mistaken_for_the_focus_chord() {
    // Neighbours in the same C0 block: Ctrl-\ (0x1C) and Ctrl-^ (0x1E) must not
    // release the keyboard.
    assert!(!is_focus_chord(as_terminal_sends(0x1c)));
    assert!(!is_focus_chord(as_terminal_sends(0x1e)));
    // Ctrl-C has to reach the harness — interrupting it is the single most
    // important key an attached pane forwards.
    assert!(!is_focus_chord(as_terminal_sends(0x03)));
    // The chord needs Ctrl: a bare `]` is text the operator is typing.
    assert!(!is_focus_chord(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::NONE
    )));
    assert!(!is_focus_chord(KeyEvent::new(
        KeyCode::Char('5'),
        KeyModifiers::NONE
    )));
}

#[test]
fn focus_defaults_to_the_chrome() {
    let focus = HarnessFocus::default();
    assert_eq!(focus, HarnessFocus::Chrome);
    assert_eq!(focus.attached_to(), None);
    assert!(!focus.is_attached_to("w_1"));
}

#[test]
fn attached_focus_names_exactly_one_session() {
    let focus = HarnessFocus::Attached("w_1".to_string());
    assert_eq!(focus.attached_to(), Some("w_1"));
    assert!(focus.is_attached_to("w_1"));
    // Not a prefix match: `w_1` must not capture keys meant for `w_10`.
    assert!(!focus.is_attached_to("w_10"));
}

#[test]
fn a_child_that_never_asked_for_the_mouse_gets_no_report() {
    // The important half of this feature. Forwarding to a harness that did not
    // enable reporting delivers `ESC [ < 6 4 ; 1 ; 1 M` as literal keystrokes
    // into whatever it has focused — usually its composer.
    assert!(mouse::wheel(
        vt100::MouseProtocolMode::None,
        vt100::MouseProtocolEncoding::Sgr,
        0,
        0,
        true
    )
    .is_none());
    assert!(mouse::wheel(
        vt100::MouseProtocolMode::None,
        vt100::MouseProtocolEncoding::Default,
        4,
        9,
        false
    )
    .is_none());
}

#[test]
fn sgr_wheel_reports_are_one_based_and_name_the_wheel_buttons() {
    // Pane-relative (0, 0) is the child's own top-left, reported as 1;1.
    assert_eq!(
        mouse::wheel(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Sgr,
            0,
            0,
            true
        )
        .unwrap(),
        b"\x1b[<64;1;1M"
    );
    // 64 is wheel-up, 65 is wheel-down.
    assert_eq!(
        mouse::wheel(
            vt100::MouseProtocolMode::PressRelease,
            vt100::MouseProtocolEncoding::Sgr,
            11,
            4,
            false
        )
        .unwrap(),
        b"\x1b[<65;12;5M"
    );
}

#[test]
fn normal_wheel_reports_offset_every_field_by_32() {
    let bytes = mouse::wheel(
        vt100::MouseProtocolMode::Press,
        vt100::MouseProtocolEncoding::Default,
        0,
        0,
        true,
    )
    .unwrap();
    // ESC [ M, then button+32, then each coordinate as (value + 1 + 32).
    assert_eq!(bytes, vec![0x1b, b'[', b'M', 32 + 64, 33, 33]);

    let down = mouse::wheel(
        vt100::MouseProtocolMode::Press,
        vt100::MouseProtocolEncoding::Default,
        9,
        2,
        false,
    )
    .unwrap();
    assert_eq!(down, vec![0x1b, b'[', b'M', 32 + 65, 42, 35]);
}

#[test]
fn normal_wheel_reports_clamp_rather_than_wrapping_past_a_byte() {
    // The normal encoding cannot express a coordinate past 223. Wrapping would
    // report a wheel notch at the opposite corner of the child's screen.
    let bytes = mouse::wheel(
        vt100::MouseProtocolMode::Press,
        vt100::MouseProtocolEncoding::Default,
        5_000,
        5_000,
        true,
    )
    .unwrap();
    assert_eq!(bytes[4], 255);
    assert_eq!(bytes[5], 255);
}

#[test]
fn every_reporting_mode_takes_wheel_notches() {
    // A wheel notch is a button press, and there is no mode that reports motion
    // but not buttons — so anything other than `None` gets a report.
    for mode in [
        vt100::MouseProtocolMode::Press,
        vt100::MouseProtocolMode::PressRelease,
        vt100::MouseProtocolMode::ButtonMotion,
        vt100::MouseProtocolMode::AnyMotion,
    ] {
        assert!(
            mouse::wheel(mode, vt100::MouseProtocolEncoding::Sgr, 1, 1, true).is_some(),
            "{mode:?} should report a wheel notch"
        );
    }
}

#[test]
fn a_wheel_at_the_extreme_of_the_coordinate_space_does_not_overflow() {
    // `col + 1` on a `u16` panics in a debug build at `u16::MAX`, and the clamp
    // that was supposed to bound it runs afterwards — too late to help. A pane
    // never gets this wide, but a panic reachable from a public encoder is a
    // panic, and the widening costs nothing.
    let sgr = mouse::wheel(
        vt100::MouseProtocolMode::Press,
        vt100::MouseProtocolEncoding::Sgr,
        u16::MAX,
        u16::MAX,
        true,
    )
    .expect("a reporting child still gets a report");
    assert_eq!(
        sgr,
        format!("\x1b[<64;{};{}M", 65_536u32, 65_536u32).as_bytes()
    );

    let normal = mouse::wheel(
        vt100::MouseProtocolMode::Press,
        vt100::MouseProtocolEncoding::Default,
        u16::MAX,
        u16::MAX,
        false,
    )
    .expect("a reporting child still gets a report");
    // Clamped to the byte ceiling rather than wrapped to the opposite corner.
    assert_eq!(normal, vec![0x1b, b'[', b'M', 32 + 65, 255, 255]);
}
