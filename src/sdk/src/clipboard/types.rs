//! Data types for the `clipboard` module.
#[allow(unused_imports)]
use super::*;
/// A clipboard writer: a binary taking the text on stdin.
pub struct Writer {
    /// The binary to run, looked up on `PATH` (`pbcopy`, `wl-copy`, …). A
    /// writer that is not installed is skipped, not an error.
    pub cmd: &'static str,
    /// Arguments passed before the text is written to the child's stdin.
    pub args: &'static [&'static str],
}

/// Which mechanisms took a copy.
///
/// Not one winner but a set, because the routes are not alternatives: on a box
/// reached over SSH inside tmux, the tmux paste buffer, a local writer and the
/// escape each reach a *different* clipboard, and which one the operator will
/// paste from is not knowable from here. Reporting all of them is also what
/// keeps a status line honest — only the [`confirmed`](Self::confirmed)
/// mechanisms actually acknowledged the text.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CopyReport {
    /// The local clipboard binary that took it, if any.
    pub writer: Option<String>,
    /// Whether the surrounding tmux server took it into its paste buffer.
    pub tmux: bool,
    /// Whether the OSC 52 escape was written to the terminal.
    ///
    /// Never a confirmation: a terminal may ignore the escape, and any tmux in
    /// the chain may drop it, with no way to tell from this side.
    pub terminal: bool,
}

impl CopyReport {
    /// Whether something acknowledged the copy, as opposed to it only having
    /// been offered to the terminal.
    pub fn confirmed(&self) -> bool {
        self.writer.is_some() || self.tmux
    }

    /// The mechanisms that took it, for a status line: `"pbcopy"`,
    /// `"tmux buffer + OSC 52"`, or just [`OSC_52`] when nothing confirmed it.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.tmux {
            parts.push("tmux buffer".to_string());
        }
        if let Some(writer) = &self.writer {
            parts.push(writer.clone());
        }
        if parts.is_empty() {
            return OSC_52.to_string();
        }
        if self.terminal {
            parts.push(OSC_52.to_string());
        }
        parts.join(" + ")
    }
}
