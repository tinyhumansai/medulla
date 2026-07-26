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
