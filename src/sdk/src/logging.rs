//! The one line-sink type every subsystem narrates through.
//!
//! The daemon, the orchestrator hub and the contact desk all need a way to emit
//! diagnostics without assuming a terminal — because the processes that host
//! them own a ratatui screen, and anything written to stdout or stderr under one
//! lands on top of the UI and never clears.
//!
//! They had each grown their own identical alias. One name means a caller can
//! build a single sink and hand it to all of them.

use std::sync::Arc;

/// A sink for one line of diagnostics.
pub type LineSink = Arc<dyn Fn(&str) + Send + Sync>;

/// A sink that writes to stderr, for callers that own their terminal.
pub fn stderr_sink() -> LineSink {
    Arc::new(|line: &str| eprintln!("{line}"))
}

/// How much of a payload a log line carries.
pub const PREVIEW_CHARS: usize = 240;

/// One line of `text`, clipped for a log.
///
/// Payload previews exist so the answer a worker produced and the answer the
/// orchestrator received can be compared without attaching a debugger. They are
/// clipped and flattened deliberately: a harness reply runs to kilobytes and
/// carries newlines, and a log that reproduces it whole is one nobody reads.
/// The character count is kept because a truncated preview cannot tell you a
/// reply was truncated, and "empty" versus "long" is usually the question.
pub fn preview(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PREVIEW_CHARS {
        return flat;
    }
    let head: String = flat.chars().take(PREVIEW_CHARS).collect();
    format!("{head}…")
}
