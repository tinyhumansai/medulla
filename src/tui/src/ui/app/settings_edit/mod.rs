//! The Config subpage's editing behaviour: which settings are editable, how to
//! read their current values out of the effective config, and how to apply and
//! persist a change.
//!
//! Editing is deliberately narrow. A config value is only exposed here when it
//! is a genuine user preference with a small, safe domain — a switch or a
//! bounded number. Paths, URLs, and credentials stay file-managed, since they
//! need validation this surface cannot offer and are set once rather than tuned.
//!
//! Every write goes through [`medulla::config::persist_setting`], which rewrites
//! only the one `[section]` key it owns. Serializing the whole [`TuiConfig`]
//! back would be wrong: the loader bakes env overrides and home-derived paths
//! into the in-memory struct, so a round-trip would pin those as if the user had
//! typed them.

use medulla::config::{MedullaConfig, MemoryConfigSection, OpencodeConfig};

use super::types::App;

mod types;

#[cfg(test)]
mod tests;

pub(crate) use types::{SettingKind, SettingRow, SettingValue};

/// A bounded, optional whole-number setting.
const fn count(min: u32, max: u32, step: u32, fallback: u32) -> SettingKind {
    SettingKind::Count {
        min,
        max,
        step,
        fallback,
        optional: true,
    }
}

impl App {
    /// The editable rows for the Config subpage, in display order.
    ///
    /// The tiny.place row appears only when a `[tinyplace]` section exists —
    /// offering peer discovery when tiny.place is switched off entirely would
    /// suggest a setting that does nothing.
    pub(crate) fn config_rows(&self) -> Vec<SettingRow> {
        let mut rows = vec![
            SettingRow {
                label: "Persona memory",
                section: "memory",
                key: "enabled",
                kind: SettingKind::Toggle,
                help: "Recall facts and directives across sessions.",
            },
            SettingRow {
                label: "Update check",
                section: "update",
                key: "check",
                kind: SettingKind::Toggle,
                help: "Check for a newer Medulla release on startup.",
            },
        ];
        if self.loaded.config.tinyplace.is_some() {
            rows.push(SettingRow {
                label: "Auto-discover peers",
                section: "tinyplace",
                key: "autoDiscoverPeers",
                kind: SettingKind::Toggle,
                help: "Find tiny.place agents automatically instead of only configured peers.",
            });
        }
        rows.extend([
            SettingRow {
                label: "Max passes",
                section: "medulla",
                key: "maxPasses",
                kind: count(1, 50, 1, 4),
                help: "Orchestration passes before a cycle stops.",
            },
            SettingRow {
                label: "Max steps",
                section: "medulla",
                key: "maxSteps",
                kind: count(1, 500, 4, 24),
                help: "Total steps a single cycle may take.",
            },
            SettingRow {
                label: "Max depth",
                section: "medulla",
                key: "maxDepth",
                kind: count(1, 10, 1, 3),
                help: "How deeply sub-agents may delegate further work.",
            },
            SettingRow {
                label: "Max tokens",
                section: "medulla",
                key: "maxTokens",
                kind: count(256, 200_000, 1024, 8192),
                help: "Token ceiling for a single model response.",
            },
            SettingRow {
                label: "Context window",
                section: "medulla",
                key: "contextWindowTokens",
                kind: count(4096, 1_000_000, 4096, 32_000),
                help: "Window size used when deciding to compress context.",
            },
            SettingRow {
                label: "Worker concurrency",
                section: "opencode",
                key: "maxConcurrency",
                kind: SettingKind::Count {
                    min: 1,
                    max: 32,
                    step: 1,
                    fallback: 4,
                    // The field is a plain u32 with a serde default, so it is
                    // always set; there is no "auto" state to fall back to.
                    optional: false,
                },
                help: "Worker harness processes Medulla may run at once.",
            },
        ]);
        rows
    }

    /// The current value of `row`, read from the effective config.
    pub(crate) fn read_setting(&self, row: &SettingRow) -> SettingValue {
        let cfg = &self.loaded.config;
        match (row.section, row.key) {
            ("memory", "enabled") => SettingValue::Flag(
                cfg.memory
                    .as_ref()
                    .and_then(|m| m.enabled)
                    // Memory is opt-in: an absent section means off.
                    .unwrap_or(false),
            ),
            ("update", "check") => SettingValue::Flag(cfg.update.check),
            ("tinyplace", "autoDiscoverPeers") => SettingValue::Flag(
                cfg.tinyplace
                    .as_ref()
                    .map(|t| t.auto_discover_peers)
                    .unwrap_or(false),
            ),
            ("medulla", key) => opt_number(medulla_field(&cfg.medulla, key)),
            ("opencode", "maxConcurrency") => SettingValue::Number(
                cfg.opencode
                    .as_ref()
                    .map(|o| o.max_concurrency)
                    .unwrap_or_else(|| OpencodeConfig::default().max_concurrency),
            ),
            _ => SettingValue::Auto,
        }
    }

    /// Apply a change to the selected Config row and persist it.
    ///
    /// `delta` is `0` for a toggle/Enter, `-1`/`+1` for ←/→. Toggles flip on any
    /// input; counts step by their row's `step`, and an optional count stepped
    /// below its minimum returns to "auto" (clearing the key) rather than
    /// pinning the default.
    ///
    /// Returns the status message to surface, so the caller controls when it is
    /// shown.
    pub(crate) fn adjust_setting(&mut self, delta: i32) -> String {
        let rows = self.config_rows();
        let Some(row) = rows.get(self.config_index.min(rows.len().saturating_sub(1))) else {
            return "Config · nothing to edit".into();
        };
        let row = *row;

        let next = match row.kind {
            SettingKind::Toggle => {
                let SettingValue::Flag(current) = self.read_setting(&row) else {
                    return "Config · unexpected value".into();
                };
                SettingValue::Flag(!current)
            }
            SettingKind::Count {
                min,
                max,
                step,
                fallback,
                optional,
            } => {
                if delta == 0 {
                    return format!("Config · {} needs ← or → to change", row.label);
                }
                match self.read_setting(&row) {
                    // Stepping an unset field starts from its documented default
                    // rather than from zero.
                    SettingValue::Auto => SettingValue::Number(fallback.clamp(min, max)),
                    SettingValue::Number(current) => {
                        let raw = current as i64 + (delta as i64) * step as i64;
                        if optional && raw < min as i64 {
                            SettingValue::Auto
                        } else {
                            SettingValue::Number(raw.clamp(min as i64, max as i64) as u32)
                        }
                    }
                    SettingValue::Flag(_) => return "Config · unexpected value".into(),
                }
            }
        };

        self.write_setting(&row, next);
        self.persist_setting_now(&row, next)
    }

    /// Mirror `value` into the in-memory config so the change shows immediately,
    /// creating any optional section it belongs to.
    fn write_setting(&mut self, row: &SettingRow, value: SettingValue) {
        let cfg = &mut self.loaded.config;
        match (row.section, row.key, value) {
            ("memory", "enabled", SettingValue::Flag(on)) => {
                cfg.memory
                    .get_or_insert_with(MemoryConfigSection::default)
                    .enabled = Some(on);
            }
            ("update", "check", SettingValue::Flag(on)) => cfg.update.check = on,
            ("tinyplace", "autoDiscoverPeers", SettingValue::Flag(on)) => {
                if let Some(tp) = cfg.tinyplace.as_mut() {
                    tp.auto_discover_peers = on;
                }
            }
            ("medulla", key, value) => {
                let slot = medulla_field_mut(&mut cfg.medulla, key);
                if let Some(slot) = slot {
                    *slot = match value {
                        SettingValue::Number(n) => Some(n),
                        _ => None,
                    };
                }
            }
            ("opencode", "maxConcurrency", SettingValue::Number(n)) => {
                cfg.opencode
                    .get_or_insert_with(OpencodeConfig::default)
                    .max_concurrency = n;
            }
            _ => {}
        }
    }

    /// Write `value` to the injected config file, returning the status message.
    ///
    /// Reports the target path so it is clear which file in the layered load was
    /// written, and warns when a higher-precedence file may still override it.
    fn persist_setting_now(&self, row: &SettingRow, value: SettingValue) -> String {
        let Some(path) = &self.config_path else {
            return format!(
                "Config · {} → {} (applied live, no config path set)",
                row.label,
                value.display()
            );
        };
        let result = match value {
            SettingValue::Auto => medulla::config::clear_setting(path, row.section, row.key),
            SettingValue::Flag(on) => medulla::config::persist_setting(
                path,
                row.section,
                row.key,
                toml::Value::Boolean(on),
            ),
            SettingValue::Number(n) => medulla::config::persist_setting(
                path,
                row.section,
                row.key,
                toml::Value::Integer(n as i64),
            ),
        };
        match result {
            Err(e) => format!("Config · save failed: {e}"),
            Ok(()) => {
                let overridden = self.overriding_source(path);
                match overridden {
                    Some(other) => format!(
                        "Config · {} → {} (saved to {}, but {other} still overrides it)",
                        row.label,
                        value.display(),
                        path.display()
                    ),
                    None => format!(
                        "Config · {} → {} (saved to {})",
                        row.label,
                        value.display(),
                        path.display()
                    ),
                }
            }
        }
    }

    /// The highest-precedence config source that outranks `path`, if any.
    ///
    /// Settings are written to the user-global file, but a project-local config
    /// layered on top would win. Naming it beats silently saving a value the
    /// user will not see take effect.
    fn overriding_source(&self, path: &std::path::Path) -> Option<String> {
        let target = path.to_string_lossy();
        let position = self.loaded.sources.iter().position(|s| *s == target)?;
        self.loaded.sources.get(position + 1..)?.last().cloned()
    }

    /// The Config subpage's selected row index, clamped to the current rows.
    pub(crate) fn config_row_index(&self) -> usize {
        let len = self.config_rows().len();
        self.config_index.min(len.saturating_sub(1))
    }

    /// Move the Config subpage cursor by one row.
    pub(crate) fn move_config_index(&mut self, up: bool) {
        let max = self.config_rows().len().saturating_sub(1);
        self.config_index = if up {
            self.config_row_index().saturating_sub(1)
        } else {
            (self.config_row_index() + 1).min(max)
        };
    }
}

/// Wrap an optional number as a [`SettingValue`].
fn opt_number(value: Option<u32>) -> SettingValue {
    match value {
        Some(n) => SettingValue::Number(n),
        None => SettingValue::Auto,
    }
}

/// Read one of [`MedullaConfig`]'s optional limits by its camelCase key.
fn medulla_field(cfg: &MedullaConfig, key: &str) -> Option<u32> {
    match key {
        "maxPasses" => cfg.max_passes,
        "maxSteps" => cfg.max_steps,
        "maxDepth" => cfg.max_depth,
        "maxTasksPerDelegate" => cfg.max_tasks_per_delegate,
        "maxTokens" => cfg.max_tokens,
        "contextWindowTokens" => cfg.context_window_tokens,
        _ => None,
    }
}

/// The mutable slot for one of [`MedullaConfig`]'s optional limits.
fn medulla_field_mut<'a>(cfg: &'a mut MedullaConfig, key: &str) -> Option<&'a mut Option<u32>> {
    match key {
        "maxPasses" => Some(&mut cfg.max_passes),
        "maxSteps" => Some(&mut cfg.max_steps),
        "maxDepth" => Some(&mut cfg.max_depth),
        "maxTasksPerDelegate" => Some(&mut cfg.max_tasks_per_delegate),
        "maxTokens" => Some(&mut cfg.max_tokens),
        "contextWindowTokens" => Some(&mut cfg.context_window_tokens),
        _ => None,
    }
}
