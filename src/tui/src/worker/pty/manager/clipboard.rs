//! Passing a harness's clipboard writes on to the operator's terminal.
//!
//! A harness runs on a pty we allocated, so *we* are its terminal: when it emits
//! OSC 52 to copy something, the escape reaches this process and stops. It has
//! to be carried the rest of the way by hand, because nothing else will —
//! `vt100` drops OSC 52 while parsing, and even if it did not, a child's bytes
//! become a screen grid here rather than being replayed to our own stdout.
//!
//! The rest of the way can be several hops: a medulla driven inside tmux, over
//! SSH, inside another tmux. [`medulla::clipboard::copy_for_operator`] is what
//! knows how to cross those, so forwarding is a matter of handing it the text
//! and writing what it produces to our terminal.

use std::io::{IsTerminal, Write};
use std::sync::Arc;

use super::types::ClipboardSink;

/// Forward a harness's copy to whoever is looking at this process.
///
/// Skipped entirely when our stdout is not a terminal: a worker daemon under
/// systemd or with its output piped has nobody to hand an escape to, and writing
/// one anyway would put a control sequence into a log file.
///
/// Blocking is acceptable here and only here — this runs on a thread of its own
/// (see [`PtyManager::forward_clipboard`](super::PtyManager::forward_clipboard))
/// because `copy_for_operator` spawns `pbcopy`/`tmux` and waits on them, which
/// on the reader thread would stall the harness's own output.
pub(super) fn to_operator(text: &str) {
    if !std::io::stdout().is_terminal() {
        return;
    }
    medulla::clipboard::copy_for_operator(text, medulla::clipboard::current_platform(), |seq| {
        let mut out = std::io::stdout();
        let _ = out.write_all(seq.as_bytes());
        let _ = out.flush();
    });
}

/// The sink every manager gets unless a caller (a test) substitutes one.
pub(super) fn default_sink() -> ClipboardSink {
    Arc::new(to_operator)
}
