//! Command-line flag parsing for `medulla daemon`: the permissive [`Flags`]
//! tokenizer (values, repeatable comma-lists, and boolean switches) and
//! [`parse_provider`], the wire-name → [`HarnessProvider`] mapper. Consumed by
//! [`super::entry`] to build the daemon configuration.

use std::collections::{HashMap, HashSet};

use crate::tinyplace_support::HarnessProvider;

use super::providers::DAEMON_PROVIDERS;

/// Flags that take no value (their presence is the value).
const BOOL_FLAGS: &[&str] = &[
    "dangerously-skip-permissions",
    "once",
    "no-onboard",
    "reonboard",
];

/// A parsed `--flag [value]` bag: repeatable string values plus a set of
/// present boolean switches.
#[derive(Default)]
pub(super) struct Flags {
    /// Repeatable `--name value` entries, in order.
    values: HashMap<String, Vec<String>>,
    /// Present boolean switches from [`BOOL_FLAGS`].
    bools: HashSet<String>,
}

impl Flags {
    /// Parse `args` into [`Flags`]. Every token must be a `--name`; boolean
    /// flags stand alone, all others consume the following token as their value.
    /// A dangling `--name` or a bare positional is an error.
    pub(super) fn parse(args: &[String]) -> Result<Self, String> {
        let mut flags = Flags::default();
        let mut index = 0;
        while index < args.len() {
            let token = &args[index];
            let name = token
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument: {token}"))?;
            if BOOL_FLAGS.contains(&name) {
                flags.bools.insert(name.to_string());
                index += 1;
            } else {
                let value = args
                    .get(index + 1)
                    .cloned()
                    .ok_or_else(|| format!("--{name} needs a value"))?;
                flags
                    .values
                    .entry(name.to_string())
                    .or_default()
                    .push(value);
                index += 2;
            }
        }
        Ok(flags)
    }

    /// The last value supplied for `name` (later wins), if any.
    pub(super) fn string(&self, name: &str) -> Option<String> {
        self.values.get(name).and_then(|v| v.last().cloned())
    }

    /// All values for `name`, flattening comma-separated and repeated entries
    /// and dropping blanks.
    pub(super) fn list(&self, name: &str) -> Option<Vec<String>> {
        self.values.get(name).map(|values| {
            values
                .iter()
                .flat_map(|value| value.split(','))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
    }

    /// Parse `name` as a `u64`, or `None` when absent; a non-numeric value errors.
    pub(super) fn number(&self, name: &str) -> Result<Option<u64>, String> {
        match self.string(name) {
            Some(raw) => raw
                .parse::<u64>()
                .map(Some)
                .map_err(|_| format!("--{name} must be a non-negative integer (got {raw})")),
            None => Ok(None),
        }
    }

    /// Parse `name` as a strictly positive `u64`, falling back to `fallback`
    /// when absent; zero is rejected.
    pub(super) fn positive(&self, name: &str, fallback: u64) -> Result<u64, String> {
        match self.number(name)? {
            Some(0) => Err(format!("--{name} must be a positive integer (got 0)")),
            Some(value) => Ok(value),
            None => Ok(fallback),
        }
    }

    /// Whether boolean switch `name` was supplied.
    pub(super) fn is_set(&self, name: &str) -> bool {
        self.bools.contains(name)
    }
}

/// Map a wire provider name to a [`HarnessProvider`], erroring with the set of
/// known names on failure.
pub(super) fn parse_provider(value: &str) -> Result<HarnessProvider, String> {
    HarnessProvider::from_wire(value).ok_or_else(|| {
        format!(
            "unknown provider \"{value}\" (expected: {})",
            DAEMON_PROVIDERS
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}
