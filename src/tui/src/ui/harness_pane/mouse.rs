//! Encoding a pointer event as the mouse report a terminal would have sent.
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
/// Added to a button code to mark the report as motion with it held down.
const MOTION_BIT: u8 = 32;
/// The normal encoding's "a button came up" code.
///
/// It does not say *which* — that encoding has no room for it, and every
/// xterm-compatible application knows to pair a release with the press it is
/// still holding. SGR does name the button, which is why it is handled apart.
const BUTTON_RELEASE: u8 = 3;

/// Which physical button a pointer event names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    /// The primary button.
    Left,
    /// The wheel button.
    Middle,
    /// The secondary button.
    Right,
}

impl Button {
    /// The protocol's code for this button, before any motion bit or offset.
    fn code(self) -> u8 {
        match self {
            Button::Left => 0,
            Button::Middle => 1,
            Button::Right => 2,
        }
    }
}

/// What the pointer did with that button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    /// The button went down.
    Press,
    /// It came back up.
    Release,
    /// The pointer moved to another cell while it was held.
    Drag,
}

/// Whether a child in `mode` wants to hear about `motion` at all.
///
/// The modes are cumulative, and forwarding past what was asked for is not a
/// harmless extra: an application that enabled press-only reporting has no
/// parser for a release, so the report reaches it as literal keystrokes.
pub fn reports(mode: MouseProtocolMode, motion: Motion) -> bool {
    match (mode, motion) {
        (MouseProtocolMode::None, _) => false,
        // X10 (DECSET 9) reports the press and nothing else.
        (MouseProtocolMode::Press, Motion::Press) => true,
        (MouseProtocolMode::Press, _) => false,
        // VT200 (1000) adds the release but still no motion.
        (MouseProtocolMode::PressRelease, Motion::Drag) => false,
        (MouseProtocolMode::PressRelease, _) => true,
        // 1002 and 1003 take everything a button can do. Matched as a
        // catch-all so a mode added to the enum later defaults to forwarding
        // rather than to silently dropping the operator's clicks.
        _ => true,
    }
}

/// Encode one button event for a child using `mode` and `encoding`.
///
/// `col` and `row` carry the same meaning as in [`wheel`]: zero-based and
/// relative to the harness screen.
///
/// Returns `None` when the child does not want this event — either it never
/// enabled mouse reporting, or it enabled a mode that does not cover this
/// motion. Callers must not fall back to sending anything else in its place.
pub fn button(
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    col: u16,
    row: u16,
    button: Button,
    motion: Motion,
) -> Option<Vec<u8>> {
    if !reports(mode, motion) {
        return None;
    }
    let held = if motion == Motion::Drag {
        button.code() + MOTION_BIT
    } else {
        button.code()
    };
    Some(match encoding {
        MouseProtocolEncoding::Sgr => sgr(held, col, row, motion != Motion::Release),
        // The normal encoding cannot name the button that was released, so a
        // release is always the same code whichever button it was.
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            let bits = if motion == Motion::Release {
                BUTTON_RELEASE
            } else {
                held
            };
            normal(bits, col, row)
        }
    })
}

/// An SGR (DECSET 1006) report: decimal, 1-based, unbounded.
///
/// `press` picks the final byte — `M` for a press or drag, `m` for a release.
/// That final letter is the *only* thing distinguishing the two in this
/// encoding, which is why it is a parameter rather than baked in.
fn sgr(button: u8, col: u16, row: u16, press: bool) -> Vec<u8> {
    format!(
        "\x1b[<{button};{};{}{}",
        u32::from(col) + 1,
        u32::from(row) + 1,
        if press { 'M' } else { 'm' }
    )
    .into_bytes()
}

/// A normal (X10) report: `ESC [ M Cb Cx Cy`, every field a byte offset by 32.
fn normal(button: u8, col: u16, row: u16) -> Vec<u8> {
    // Widened before the `+ 1`, not after. The clamp below bounds the *result*,
    // which is no help if the arithmetic reaching it has already overflowed —
    // `u16::MAX + 1` panics in a debug build before the `.min()` is ever
    // evaluated.
    let offset = u32::from(NORMAL_OFFSET);
    let ceiling = u32::from(NORMAL_MAX_COORD) + offset;
    let cx = (u32::from(col) + 1 + offset).min(ceiling);
    let cy = (u32::from(row) + 1 + offset).min(ceiling);
    vec![
        0x1b,
        b'[',
        b'M',
        (NORMAL_OFFSET as u8).saturating_add(button),
        cx as u8,
        cy as u8,
    ]
}

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
        // A wheel notch has no release, so there is no matching `m` report to
        // send after it — it is always encoded as a press.
        MouseProtocolEncoding::Sgr => sgr(button, col, row, true),
        // UTF-8 encoding (DECSET 1005) is a dead end almost nothing negotiates,
        // and its multi-byte coordinates decode ambiguously. Treating it as the
        // normal encoding is what xterm-compatible terminals do for the low
        // coordinates that fit in both, which is every coordinate a pane of this
        // size produces.
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => normal(button, col, row),
    })
}
