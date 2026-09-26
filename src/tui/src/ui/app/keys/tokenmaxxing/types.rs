//! Result type for TokenMaxxxing-specific keyboard handling.
#[allow(unused_imports)]
use super::*;

/// Whether the TokenMaxxxing pages consumed a key and any command they emitted.
pub(in super::super) enum TokenMaxxxingKey {
    /// The key belonged to the TokenMaxxxing navigation surface.
    Handled(Option<Cmd>),
    /// The key should continue to global bindings.
    Unhandled,
}
