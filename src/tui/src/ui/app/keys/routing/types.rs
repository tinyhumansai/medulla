//! Data types for the `routing` module.
#[allow(unused_imports)]
use super::*;
/// Whether Routing consumed a key and its optional runtime command.
pub(in super::super) enum RoutingKey {
    /// Routing handled the key.
    Handled(Option<Cmd>),
    /// A structural/global binding may handle the key.
    Unhandled,
}
