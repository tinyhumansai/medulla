//! Data types for the `memory` module.
#[allow(unused_imports)]
use super::*;
/// Whether Memory consumed a key and its optional service command.
pub(in super::super) enum MemoryKey {
    /// Memory handled the key.
    Handled(Option<Cmd>),
    /// A structural/global binding may handle the key.
    Unhandled,
}
