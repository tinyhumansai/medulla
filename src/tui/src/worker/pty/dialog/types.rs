//! Data types for the `dialog` module.
#[allow(unused_imports)]
use super::*;
/// A startup dialog that has to be answered before the harness will take work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockingDialog {
    /// What the harness is waiting for, in the operator's terms.
    pub what: &'static str,
    /// What to do about it.
    pub remedy: &'static str,
    /// The keystrokes that safely dismiss this dialog, if the worker can clear
    /// it itself, as a sequence of raw byte writes (one write per key, so the
    /// injector can pace them and let each land before the next).
    ///
    /// `None` means report-only: the worker recognises the dialog but will not
    /// answer it, because the right fix is the [`remedy`](Self::remedy), not a
    /// blind keypress into someone else's modal.
    pub dismissal: Option<&'static [&'static [u8]]>,
}
