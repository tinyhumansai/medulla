//! The TUI color theme: a small set of roles that drive selection highlighting,
//! panel chrome, and accents. Defaults to the established medulla blue (Cyan).
//!
//! Colors come from the optional `[theme]` config section (named ratatui colors
//! or `#rrggbb` hex), with per-field fallback to the defaults. The Appearance
//! subpage edits the live theme and persists just the `[theme]` keys back into
//! the user-global config via [`persist_theme`].

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};

use medulla::config::ThemeConfig;

/// The editable theme roles, in Appearance-editor order.
pub const THEME_ROLES: [&str; 4] = ["primary", "accent", "selection_fg", "dim_border"];

/// A curated palette the Appearance editor cycles through. Named colors keep the
/// persisted config readable; a `#rrggbb` custom value from config is folded in
/// as an extra step at runtime.
pub const PALETTE: [Color; 10] = [
    Color::Cyan,
    Color::LightCyan,
    Color::Blue,
    Color::LightBlue,
    Color::Magenta,
    Color::Green,
    Color::Yellow,
    Color::Red,
    Color::White,
    Color::DarkGray,
];

impl Default for Theme {
    fn default() -> Self {
        Theme {
            primary: Color::Cyan,
            accent: Color::Magenta,
            selection_fg: Color::Black,
            dim_border: Color::DarkGray,
        }
    }
}

impl Theme {
    /// Resolve a theme from config, falling back per-field to the default when a
    /// field is absent or fails to parse.
    pub fn from_config(cfg: &ThemeConfig) -> Self {
        let d = Theme::default();
        let pick = |s: &Option<String>, fallback: Color| {
            s.as_deref().and_then(parse_color).unwrap_or(fallback)
        };
        Theme {
            primary: pick(&cfg.primary, d.primary),
            accent: pick(&cfg.accent, d.accent),
            selection_fg: pick(&cfg.selection_fg, d.selection_fg),
            dim_border: pick(&cfg.dim_border, d.dim_border),
        }
    }

    /// The single, unified selected-row style: primary background with a
    /// contrasting foreground. Applied to every selected/highlighted row.
    pub fn selection(&self) -> Style {
        Style::default()
            .bg(self.primary)
            .fg(self.selection_fg)
            .add_modifier(Modifier::BOLD)
    }

    /// The color for editable role `index` (see [`THEME_ROLES`]).
    pub fn role(&self, index: usize) -> Color {
        match index {
            0 => self.primary,
            1 => self.accent,
            2 => self.selection_fg,
            _ => self.dim_border,
        }
    }

    fn set_role(&mut self, index: usize, color: Color) {
        match index {
            0 => self.primary = color,
            1 => self.accent = color,
            2 => self.selection_fg = color,
            _ => self.dim_border = color,
        }
    }

    /// Advance role `index` to the next (or previous) palette entry, treating any
    /// current custom color as a virtual step so it is not lost on the first move.
    pub fn cycle_role(&mut self, index: usize, forward: bool) {
        let current = self.role(index);
        let pos = PALETTE.iter().position(|c| *c == current);
        let next = match pos {
            Some(p) => {
                let len = PALETTE.len();
                if forward {
                    PALETTE[(p + 1) % len]
                } else {
                    PALETTE[(p + len - 1) % len]
                }
            }
            None => {
                // Custom (e.g. hex) value: step into the palette from an edge.
                if forward {
                    PALETTE[0]
                } else {
                    PALETTE[PALETTE.len() - 1]
                }
            }
        };
        self.set_role(index, next);
    }
}

/// Parse a color from a ratatui color name (case-insensitive) or a `#rrggbb` hex
/// string. Returns `None` for anything unrecognized.
pub fn parse_color(input: &str) -> Option<Color> {
    let s = input.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some(Color::Rgb(r, g, b));
    }
    Some(match s.to_ascii_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "white" => Color::White,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "reset" => Color::Reset,
        _ => return None,
    })
}

/// Render a color back to a config-friendly string: a named color when it maps to
/// one, otherwise `#rrggbb`.
pub fn color_to_string(color: Color) -> String {
    match color {
        Color::Black => "black".into(),
        Color::Red => "red".into(),
        Color::Green => "green".into(),
        Color::Yellow => "yellow".into(),
        Color::Blue => "blue".into(),
        Color::Magenta => "magenta".into(),
        Color::Cyan => "cyan".into(),
        Color::Gray => "gray".into(),
        Color::DarkGray => "darkgray".into(),
        Color::White => "white".into(),
        Color::LightRed => "lightred".into(),
        Color::LightGreen => "lightgreen".into(),
        Color::LightYellow => "lightyellow".into(),
        Color::LightBlue => "lightblue".into(),
        Color::LightMagenta => "lightmagenta".into(),
        Color::LightCyan => "lightcyan".into(),
        Color::Reset => "reset".into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => format!("indexed{i}"),
    }
}

/// Read-modify-write the `[theme]` section of a TOML config file at `path`,
/// preserving every other section. Missing files (and parent dirs) are created.
/// Comments are not preserved (the `toml` crate is value-based).
pub fn persist_theme(path: &Path, theme: &Theme) -> anyhow::Result<()> {
    use toml::Value;

    let mut section = toml::Table::new();
    section.insert(
        "primary".into(),
        Value::String(color_to_string(theme.primary)),
    );
    section.insert(
        "accent".into(),
        Value::String(color_to_string(theme.accent)),
    );
    section.insert(
        "selectionFg".into(),
        Value::String(color_to_string(theme.selection_fg)),
    );
    section.insert(
        "dimBorder".into(),
        Value::String(color_to_string(theme.dim_border)),
    );
    medulla::config::persist_section(path, "theme", section)
}

#[cfg(test)]
mod tests;

mod types;
pub use types::Theme;
