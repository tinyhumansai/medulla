//! Streaming OSC 52 capture, independent of `vte`'s fixed-size OSC buffer.
//!
//! `vte` 0.11 defaults to its `no_std` feature (see the workspace lockfile),
//! which backs [`vte::Parser`]'s OSC accumulator with a **fixed 1024-byte**
//! `ArrayVec`. Once a real OSC payload — `52;c;<base64>` — grows past that,
//! `vte` silently stops appending and still dispatches at the terminator with
//! whatever fit, so [`osc52_from_params`](medulla::clipboard::tmux::osc52_from_params)
//! sees a truncated payload and (almost always) fails to decode it. A harness
//! copying more than roughly 750 bytes of text — an ordinary code selection —
//! would lose the copy entirely.
//!
//! This runs a second, much simpler state machine over the same byte stream,
//! tracking only `ESC ] ... ST`/`BEL`, with a buffer generous enough that no
//! realistic clipboard payload is ever truncated. [`super::screen`] feeds it
//! every byte alongside the `vte` parser; on a complete OSC 52 write it
//! supersedes whatever (possibly truncated) result `vte`'s own dispatch
//! produced for the same escape.

use medulla::clipboard::tmux::osc52_from_params;

/// Bound on one OSC 52 payload capture, in raw (pre-decode) bytes.
///
/// Far above anything a person copies by hand — `vte`'s own buffer already
/// truncates at 1024 bytes — so this exists only to keep a child that never
/// terminates its OSC from growing this buffer without limit.
const MAX_OSC52_BYTES: usize = 8 * 1024 * 1024;

/// Where [`Osc52Scanner::advance`] is in the escape it is watching for.
#[derive(Default, PartialEq, Eq)]
enum ScanState {
    /// Not inside anything the scanner cares about.
    #[default]
    Idle,
    /// Just saw `ESC`; a `]` next starts an OSC.
    SawEsc,
    /// Accumulating an OSC body.
    InOsc,
    /// Inside an OSC body, just saw `ESC`; a `\` next is String Terminator.
    OscSawEsc,
}

/// Watches a PTY byte stream for complete `OSC 52` clipboard writes.
///
/// Kept beside, not instead of, `vte`'s own OSC handling: `vte` still owns
/// every other escape and the screen grid, and this only exists to give OSC
/// 52 specifically a buffer `vte`'s `no_std` build does not have. See the
/// module docs for why.
#[derive(Default)]
pub(super) struct Osc52Scanner {
    state: ScanState,
    buf: Vec<u8>,
}

impl Osc52Scanner {
    /// Feed one byte, returning the clipboard text if this byte completed a
    /// valid OSC 52 write.
    pub(super) fn advance(&mut self, byte: u8) -> Option<String> {
        match self.state {
            ScanState::Idle => {
                if byte == 0x1b {
                    self.state = ScanState::SawEsc;
                }
                None
            }
            ScanState::SawEsc => {
                self.state = if byte == b']' {
                    self.buf.clear();
                    ScanState::InOsc
                } else {
                    ScanState::Idle
                };
                None
            }
            ScanState::InOsc => self.advance_in_osc(byte),
            ScanState::OscSawEsc => {
                self.state = ScanState::Idle;
                if byte == b'\\' {
                    self.finish()
                } else {
                    // Not a valid ST: whatever this is, our OSC never
                    // terminated cleanly, so there is nothing to dispatch.
                    self.buf.clear();
                    None
                }
            }
        }
    }

    fn advance_in_osc(&mut self, byte: u8) -> Option<String> {
        match byte {
            0x07 => {
                self.state = ScanState::Idle;
                self.finish()
            }
            0x1b => {
                self.state = ScanState::OscSawEsc;
                None
            }
            _ if self.buf.len() >= MAX_OSC52_BYTES => {
                // Over budget: this cannot be a clipboard write worth
                // forwarding, so drop the capture but keep watching for the
                // terminator so the scanner does not get stuck reading this
                // OSC's tail as the start of the next escape.
                self.state = ScanState::Idle;
                self.buf.clear();
                None
            }
            _ => {
                self.buf.push(byte);
                None
            }
        }
    }

    /// The OSC body is complete; parse it as OSC 52 params if it is one.
    fn finish(&mut self) -> Option<String> {
        let params: Vec<&[u8]> = self.buf.split(|&b| b == b';').collect();
        let result = osc52_from_params(&params);
        self.buf.clear();
        result
    }
}
