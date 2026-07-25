//! Clipboard writers: try a platform binary (pbcopy / clip / wl-copy / xclip /
//! xsel) then fall back to OSC 52 (hand the text to the terminal). OSC 52 is the
//! only mechanism that survives SSH, so it backstops rather than replaces the
//! spawn path.

use std::io::Write;
use std::process::{Command, Stdio};

use base64::Engine;

/// Sentinel returned when the copy fell through to the OSC 52 backstop. Unlike a
/// writer exiting 0 this is "handed to the terminal", NOT "copied" — callers must
/// not report it as a completed copy.
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
            return false;
        }
    }
    matches!(child.wait(), Ok(status) if status.success())
}

/// Copy `text`, returning the mechanism that took it (the writer command, or
/// [`OSC_52`]). `emit_osc` receives the escape sequence for the fallback path
/// (write it to the terminal).
pub fn copy_to_clipboard<F: FnMut(&str)>(text: &str, platform: &str, mut emit_osc: F) -> String {
    for w in writers(platform) {
        if pipe_to(w.cmd, w.args, text) {
            return w.cmd.to_string();
        }
    }
    emit_osc(&osc52(text));
    OSC_52.to_string()
}

/// The current OS name for [`writers`] / [`copy_to_clipboard`].
pub fn current_platform() -> &'static str {
    std::env::consts::OS
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;

mod types;
pub use types::Writer;
