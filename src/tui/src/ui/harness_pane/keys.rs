//! Encoding a crossterm [`KeyEvent`] back into the bytes a terminal would have
//! sent.
//!
//! Crossterm's job is to turn the escape sequences arriving on stdin into typed
//! events. An attached pane needs the inverse: we are the harness's terminal, so
//! whatever the operator pressed has to reach it in the wire form its own input
//! layer is parsing. Anything we fail to encode is a key that silently does
//! nothing inside the harness, which reads as a hung program.
//!
//! The vocabulary is xterm's, because that is what `TERM=xterm-256color`
//! promises the child (see the PTY launch env) and what both Claude Code and
//! Codex parse. Two conventions are worth stating because they are the ones
//! that look wrong:
//!
//! - **Enter is `\r`, not `\n`.** The child's line discipline is in raw mode and
//!   its TUI reads carriage return as Enter; a newline types a literal newline
//!   into the composer instead of submitting.
//! - **Backspace is `\x7f` (DEL), not `\x08`.** Terminals have sent DEL for the
//!   backspace key since DEC, and a harness that gets `\x08` treats it as
//!   Ctrl-H.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Whether `key` is the reserved attach/detach chord, `Ctrl-]`.
///
/// Two spellings, because a terminal has two ways of telling us. Under the
/// legacy encoding the operator's `Ctrl-]` arrives as the single byte `0x1D`,
/// and crossterm decodes the `0x1C..=0x1F` block by counting up from `'4'` —
/// so it surfaces as `Ctrl+'5'`, never as `Ctrl+']'`. Under the kitty keyboard
/// protocol the real key is reported and it arrives as `Ctrl+']'`. Matching only
/// the second is the mistake this function exists to prevent: the chord then
/// silently does nothing on every ordinary terminal.
///
/// The cost is that `Ctrl-5` is also the chord on legacy terminals. That is not
/// a choice we can make differently — both keys send the same byte, so nothing
/// downstream can tell them apart.
pub fn is_focus_chord(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
}

/// Encode one key press as the bytes a terminal would send for it.
///
/// Returns `None` for a key with no terminal representation — modifier presses
/// on their own, and key *release* events on platforms that report them. Sending
/// nothing is correct there; a release encoded as a press would double every
/// keystroke.
pub fn encode(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    let mut bytes = match key.code {
        KeyCode::Char(c) if ctrl => vec![control_byte(c)?],
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        // Shift-Tab is its own sequence rather than a modified Tab: harnesses
        // use it to cycle backwards through modes, and CSI Z is how they read it.
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Null => vec![0],
        // Cursor and editing keys. The modified forms use the `CSI 1 ; m X`
        // encoding, which is why the plain and modified cases share a tail.
        KeyCode::Up => cursor(b'A', ctrl, alt, shift),
        KeyCode::Down => cursor(b'B', ctrl, alt, shift),
        KeyCode::Right => cursor(b'C', ctrl, alt, shift),
        KeyCode::Left => cursor(b'D', ctrl, alt, shift),
        KeyCode::Home => cursor(b'H', ctrl, alt, shift),
        KeyCode::End => cursor(b'F', ctrl, alt, shift),
        KeyCode::Insert => tilde(2, ctrl, alt, shift),
        KeyCode::Delete => tilde(3, ctrl, alt, shift),
        KeyCode::PageUp => tilde(5, ctrl, alt, shift),
        KeyCode::PageDown => tilde(6, ctrl, alt, shift),
        KeyCode::F(n) => function(n)?,
        // Modifier presses, media keys, and release/repeat-only codes have no
        // byte form. Dropping them is what a real terminal does too.
        _ => return None,
    };

    // Alt is the ESC prefix, applied last so it wraps whatever the key encoded
    // to. Skipped for sequences that are already CSI-introduced: those carry
    // their modifier in the parameter, and prefixing them produces `ESC ESC [`,
    // which harnesses read as a stray Escape followed by a garbled sequence.
    if alt && !bytes.starts_with(b"\x1b") {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

/// The control byte for `Ctrl` plus a character, or `None` when the combination
/// has no C0 encoding.
///
/// The mapping is the historical one: Ctrl clears the top three bits, so
/// `Ctrl-A` is 1 and `Ctrl-Z` is 26, and the four symbols after `Z` continue the
/// run up to 31. `Ctrl-Space` and `Ctrl-@` are both NUL, which is how a terminal
/// has always sent them.
fn control_byte(c: char) -> Option<u8> {
    let upper = c.to_ascii_uppercase();
    match upper {
        'A'..='Z' => Some(upper as u8 - b'A' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' | '?' => Some(0x1f),
        ' ' | '@' => Some(0),
        // Ctrl with an unmapped character (a digit, say) sends the character
        // itself on every terminal worth matching.
        _ => Some(c as u32 as u8).filter(|_| c.is_ascii()),
    }
}

/// A cursor-style key: `CSI X` unmodified, `CSI 1 ; m X` with modifiers.
fn cursor(final_byte: u8, ctrl: bool, alt: bool, shift: bool) -> Vec<u8> {
    match modifier_param(ctrl, alt, shift) {
        Some(m) => format!("\x1b[1;{m}{}", final_byte as char).into_bytes(),
        None => vec![0x1b, b'[', final_byte],
    }
}

/// A `CSI n ~` key: `CSI n ~` unmodified, `CSI n ; m ~` with modifiers.
fn tilde(n: u8, ctrl: bool, alt: bool, shift: bool) -> Vec<u8> {
    match modifier_param(ctrl, alt, shift) {
        Some(m) => format!("\x1b[{n};{m}~").into_bytes(),
        None => format!("\x1b[{n}~").into_bytes(),
    }
}

/// The xterm modifier parameter, or `None` when no modifier is held.
///
/// The encoding is `1 + shift + 2*alt + 4*ctrl`, so plain is 1 (omitted),
/// Shift is 2, Alt 3, Ctrl 5, and the combinations fall out of the sum.
fn modifier_param(ctrl: bool, alt: bool, shift: bool) -> Option<u8> {
    let bits = u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    (bits != 0).then_some(bits + 1)
}

/// Function keys. F1–F4 are the SS3 forms; F5 upwards are `CSI n ~`, with the
/// well-known gaps in the numbering that xterm has carried since the VT220.
fn function(n: u8) -> Option<Vec<u8>> {
    let seq = match n {
        1 => "\x1bOP".to_string(),
        2 => "\x1bOQ".to_string(),
        3 => "\x1bOR".to_string(),
        4 => "\x1bOS".to_string(),
        5 => "\x1b[15~".to_string(),
        6..=10 => format!("\x1b[{}~", n + 11),
        11 | 12 => format!("\x1b[{}~", n + 12),
        _ => return None,
    };
    Some(seq.into_bytes())
}
