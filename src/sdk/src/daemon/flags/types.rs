//! Data types for the `flags` module.
#[allow(unused_imports)]
use super::*;
/// A parsed `--flag [value]` bag: repeatable string values plus a set of
/// present boolean switches.
#[derive(Default)]
pub(in super::super) struct Flags {
    /// Repeatable `--name value` entries, in order.
    pub(super) values: HashMap<String, Vec<String>>,
    /// Present boolean switches from [`BOOL_FLAGS`].
    pub(super) bools: HashSet<String>,
}
