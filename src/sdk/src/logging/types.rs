//! Data types for the `logging` module.
#[allow(unused_imports)]
use super::*;
/// A sink for one line of diagnostics.
pub type LineSink = Arc<dyn Fn(&str) + Send + Sync>;
