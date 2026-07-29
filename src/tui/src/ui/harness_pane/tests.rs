//! Unit tests for the embedded harness pane: key encoding and focus.
//!
//! The encoder is where a mistake is invisible until an operator is sitting in
//! front of a harness that ignores their arrow keys, so every family it emits is
//! pinned to a literal byte sequence rather than to a round-trip through
//! crossterm — a round-trip would pass just as happily if both directions were
//! wrong together.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::keys::encode;
use super::HarnessFocus;

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
