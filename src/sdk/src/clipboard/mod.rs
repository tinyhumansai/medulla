//! Clipboard writers: the surrounding tmux's paste buffer, a platform binary
//! (pbcopy / clip / wl-copy / xclip / xsel), and OSC 52 (hand the text to the
//! terminal). OSC 52 is the only mechanism that survives SSH, so it backstops
//! rather than replaces the spawn path — and [`tmux`] is what keeps it working
//! when the SSH session lands in a multiplexer instead of a terminal.

use std::io::Write;
use std::process::{Command, Stdio};

use base64::Engine;

/// Sentinel naming the terminal hand-off in [`CopyReport::describe`]. Unlike a
/// writer exiting 0 this is "handed to the terminal", NOT "copied" — callers
/// must not report it as a completed copy.
pub const OSC_52: &str = "OSC 52";

/// Clipboard binaries to try, in order. The X11/Wayland set backstops the other
/// unixes, which ship the same tools.
pub fn writers(platform: &str) -> &'static [Writer] {
    const DARWIN: &[Writer] = &[Writer {
        cmd: "pbcopy",
        args: &[],
    }];
    const WIN: &[Writer] = &[Writer {
        cmd: "clip",
        args: &[],
    }];
    const LINUX: &[Writer] = &[
        Writer {
            cmd: "wl-copy",
            args: &[],
        },
        Writer {
            cmd: "xclip",
            args: &["-selection", "clipboard"],
        },
        Writer {
            cmd: "xsel",
            args: &["--clipboard", "--input"],
        },
    ];
    match platform {
        "macos" => DARWIN,
        "windows" => WIN,
        _ => LINUX,
    }
}

/// OSC 52 escape sequence carrying `text` base64-encoded.
pub fn osc52(text: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    format!("\x1b]52;c;{b64}\x07")
}

/// Pipe `text` into `cmd` over stdin. Returns false when the binary is missing or
/// exits non-zero — never errors, so the caller can just try the next writer.
pub fn pipe_to(cmd: &str, args: &[&str], text: &str) -> bool {
    let child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            // Reap it before giving up. Returning straight away leaves the
            // helper as a zombie for the life of the process, and a TUI that
            // retries a copy would accumulate one per attempt.
            drop(stdin);
            let _ = child.wait();
            return false;
        }
    }
    matches!(child.wait(), Ok(status) if status.success())
}

/// Copy `text`, reporting which mechanisms took it. `emit_osc` receives the
/// escape sequence for the terminal hand-off (write it to the terminal).
///
/// Order is local-first: the tmux paste buffer, then a platform writer, then the
/// escape. This is the path for text an operator copies to use *on this machine*
/// — a transcript pasted into an editor — and also the path for anything too
/// large to trust to an escape, since terminals cap what an OSC 52 may carry
/// while a tmux buffer does not.
pub fn copy_to_clipboard<F: FnMut(&str)>(text: &str, platform: &str, emit_osc: F) -> CopyReport {
    copy_with(writers(platform), Tmux::from_env().as_ref(), text, emit_osc)
}

/// [`copy_to_clipboard`] over an explicit writer list and tmux context.
///
/// Split out so the paths — tmux took it, a writer took it, nothing did — can be
/// tested without depending on which clipboard binaries the test machine happens
/// to have. Selecting by platform name cannot do that: `writers("macos")` names
/// `pbcopy`, which exists on a developer's Mac and not in Linux CI, so the same
/// assertion would exercise a different branch in each place.
fn copy_with<F: FnMut(&str)>(
    writers: &[Writer],
    tmux: Option<&Tmux>,
    text: &str,
    mut emit_osc: F,
) -> CopyReport {
    let mut report = CopyReport {
        tmux: tmux.is_some_and(|tmux| tmux.load_buffer(text)),
        writer: writers
            .iter()
            .find(|w| pipe_to(w.cmd, w.args, text))
            .map(|w| w.cmd.to_string()),
        terminal: false,
    };
    if !report.confirmed() {
        emit_osc(&tmux::operator_sequence(text, tmux));
        report.terminal = true;
    }
    report
}

/// Copy `text` to the clipboard of whoever is *looking at* this program, which
/// is not always the machine running it.
///
/// [`copy_to_clipboard`] treats OSC 52 as a fallback, which is right when the
/// program and the operator share a machine. It is wrong for anything a worker
/// hands back over SSH: `pbcopy`/`xclip` would succeed on the remote box and
/// quietly fill a clipboard nobody can paste from, and because they succeeded
/// the escape would never be emitted. So here the escape goes out *first* and
/// unconditionally — by every route the surrounding tmux leaves open, see
/// [`tmux`] — and a local writer is attempted afterwards only so a locally-run
/// session still lands in a real clipboard.
pub fn copy_for_operator<F: FnMut(&str)>(text: &str, platform: &str, emit_osc: F) -> CopyReport {
    copy_for_operator_with(writers(platform), Tmux::from_env().as_ref(), text, emit_osc)
}

/// [`copy_for_operator`] over an explicit writer list and tmux context; see
/// [`copy_with`].
fn copy_for_operator_with<F: FnMut(&str)>(
    writers: &[Writer],
    tmux: Option<&Tmux>,
    text: &str,
    mut emit_osc: F,
) -> CopyReport {
    emit_osc(&tmux::operator_sequence(text, tmux));
    CopyReport {
        // The buffer of the tmux we are inside is not the operator's clipboard,
        // but it is one prefix-`]` away from being pasted, and unlike the escape
        // it needs no cooperation from any terminal in the chain.
        tmux: tmux.is_some_and(|tmux| tmux.load_buffer(text)),
        writer: writers
            .iter()
            .find(|w| pipe_to(w.cmd, w.args, text))
            .map(|w| w.cmd.to_string()),
        terminal: true,
    }
}

/// The current OS name for [`writers`] / [`copy_to_clipboard`].
pub fn current_platform() -> &'static str {
    std::env::consts::OS
}

#[cfg(test)]
mod tests;

pub mod tmux;
pub use tmux::Tmux;

mod types;
pub use types::{CopyReport, Writer};
