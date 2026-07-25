//! Data types for the `clipboard` module.
#[allow(unused_imports)]
use super::*;
/// A clipboard writer: a binary taking the text on stdin.
pub struct Writer {
    pub cmd: &'static str,
    pub args: &'static [&'static str],
}
