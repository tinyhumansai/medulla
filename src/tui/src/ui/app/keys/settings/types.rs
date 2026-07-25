//! Data types for the `settings` module.
#[allow(unused_imports)]
use super::*;
/// Whether the Settings dispatcher consumed a key, and any command it produced.
pub(in super::super) enum SettingsKey {
    /// Settings handled the key; run the enclosed command, if any.
    Handled(Box<Option<Cmd>>),
    /// Settings does not bind this key — fall through to the global bindings.
    Unhandled,
}
