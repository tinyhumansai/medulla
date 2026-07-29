//! Encoding a wheel event as the mouse report a terminal would have sent.
//!
//! The counterpart to [`keys`](super::keys), and it carries one extra rule that
//! keys do not: a terminal reports the mouse **only to an application that asked
//! it to**. Claude Code and Codex do ask — that is how their transcripts scroll
//! — but a harness that did not would receive the report as literal keystrokes,
//! and `ESC [ < 6 4 ; 4 ; 9 M` typed into its composer is a worse outcome than a
//! wheel that does nothing. So every caller checks
//! [`PtyManager::mouse_protocol`](crate::worker::pty::PtyManager::mouse_protocol)
//! first, and this module only ever encodes for a child that opted in.
//!
//! Two wire formats are in play, because terminals accumulated them:
//!
//! - **Normal** (the original X10 report): `ESC [ M Cb Cx Cy`, where every field
//!   is a byte offset by 32. It cannot express a coordinate past 223, which is
//!   why SGR exists and why this one clamps.
//! - **SGR** (DECSET 1006): `ESC [ < b ; x ; y M`, decimal and unbounded. What
//!   anything modern negotiates.

use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// The wheel-up button code in every mouse protocol.
const BUTTON_WHEEL_UP: u8 = 64;
/// The wheel-down button code.
const BUTTON_WHEEL_DOWN: u8 = 65;
/// Every field of a normal-encoding report is offset by this.
const NORMAL_OFFSET: u16 = 32;
/// The largest coordinate a normal-encoding report can carry (255 - 32).
const NORMAL_MAX_COORD: u16 = 223;

/// Encode one wheel notch for a child using `mode` and `encoding`.
///
/// `col` and `row` are **zero-based and relative to the harness screen**, not to
/// the terminal — the child believes it owns a display starting at its own
/// origin, and reporting our pane's absolute position would put the event in the
/// wrong place on its screen.
///
/// Returns `None` when the child has not enabled mouse reporting, which is the
/// case the caller must handle by scrolling our own emulator instead.
pub fn wheel(
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    col: u16,
    row: u16,
    up: bool,
) -> Option<Vec<u8>> {
    // `None` means the child never sent DECSET 1000/1002/1003. Every other mode
    // reports button presses, and a wheel notch is a press — there is no mode
    // that takes motion but not buttons.
    if mode == MouseProtocolMode::None {
        return None;
    }
    let button = if up {
        BUTTON_WHEEL_UP
    } else {
        BUTTON_WHEEL_DOWN
    };
    Some(match encoding {
        MouseProtocolEncoding::Sgr => {
            // Decimal and 1-based. `M` is a press; the wheel has no release, so
            // there is no matching `m` report to send after it.
            format!("\x1b[<{};{};{}M", button, col + 1, row + 1).into_bytes()
        }
        // UTF-8 encoding (DECSET 1005) is a dead end almost nothing negotiates,
        // and its multi-byte coordinates decode ambiguously. Treating it as the
        // normal encoding is what xterm-compatible terminals do for the low
        // coordinates that fit in both, which is every coordinate a pane of this
        // size produces.
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            let cx = (col + 1 + NORMAL_OFFSET).min(NORMAL_MAX_COORD + NORMAL_OFFSET);
            let cy = (row + 1 + NORMAL_OFFSET).min(NORMAL_MAX_COORD + NORMAL_OFFSET);
            vec![
                0x1b,
                b'[',
                b'M',
                (NORMAL_OFFSET as u8).saturating_add(button),
                cx as u8,
                cy as u8,
            ]
        }
    })
}
