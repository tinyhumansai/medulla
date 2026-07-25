//! Data types for the `composer` module.
#[allow(unused_imports)]
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub row: usize,
    pub col: usize,
}
/// A composer draft: the text plus the caret's flat char offset into it. The
/// two travel together — an edit moving one without the other would strand the
/// caret on a character the user did not type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    pub cursor: usize,
}
