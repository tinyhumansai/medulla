//! The data model for the Config subpage's editable rows: what a row edits,
//! how it is displayed, and where it is written back.

/// How a [`SettingRow`]'s value is edited and rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingKind {
    /// An on/off switch. Enter or ←/→ flips it.
    Toggle,
    /// A bounded whole number. ←/→ step it by `step`, clamped to `min..=max`.
    Count {
        /// The lowest settable value.
        min: u32,
        /// The highest settable value.
        max: u32,
        /// How far one ←/→ press moves the value.
        step: u32,
        /// The value a currently-unset field starts from when first stepped.
        fallback: u32,
        /// Whether the field may be left unset. An optional field steps below
        /// `min` into "auto", which removes the key so the layered default
        /// applies again; a required field clamps at `min`.
        optional: bool,
    },
}

/// The current value of a setting, as read from the effective config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingValue {
    /// A boolean, resolved through its default when the key is absent.
    Flag(bool),
    /// A number that is explicitly set.
    Number(u32),
    /// An optional number with no value, so the runtime default applies.
    Auto,
}

impl SettingValue {
    /// The value as shown in the editor's value column.
    pub(crate) fn display(&self) -> String {
        match self {
            SettingValue::Flag(true) => "on".into(),
            SettingValue::Flag(false) => "off".into(),
            SettingValue::Number(n) => n.to_string(),
            SettingValue::Auto => "auto".into(),
        }
    }
}

/// One editable row on the Config subpage.
///
/// A row maps a labelled control onto exactly one `section.key` pair in the TOML
/// config, so editing it is a section-scoped write rather than a re-serialization
/// of the whole config (which would bake in env- and home-derived values).
#[derive(Debug, Clone, Copy)]
pub(crate) struct SettingRow {
    /// The row's label in the editor.
    pub(crate) label: &'static str,
    /// The TOML table this setting lives in.
    pub(crate) section: &'static str,
    /// The camelCase key within that table.
    pub(crate) key: &'static str,
    /// How the value is edited.
    pub(crate) kind: SettingKind,
    /// One-line explanation shown beneath the editor for the selected row.
    pub(crate) help: &'static str,
}
