//! Data types for the `tasks` module.
#[allow(unused_imports)]
use super::*;
/// Whether Tasks consumed a key and its optional persistence command.
pub(in super::super) enum TasksKey {
    /// Tasks handled the key.
    Handled(Option<Cmd>),
    /// A structural/global binding may handle the key.
    Unhandled,
}
