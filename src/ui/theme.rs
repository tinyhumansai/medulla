//! The TUI color theme: a small set of roles that drive selection highlighting,
//! panel chrome, and accents. Defaults to the established medulla blue (Cyan).
//!
//! Colors come from the optional `[theme]` config section (named ratatui colors
//! or `#rrggbb` hex), with per-field fallback to the defaults. The Appearance
//! subpage edits the live theme and persists just the `[theme]` keys back into
//! the user-global config via [`persist_theme`].

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};

use crate::config::ThemeConfig;

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

/// The resolved color roles used across the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Selection/highlight background, brand, panel titles, and primary accents.
    pub primary: Color,
    /// Secondary accent for inline overlays (prompt/resume borders).
    pub accent: Color,
    /// Foreground drawn on top of `primary` for selected rows.
    pub selection_fg: Color,
    /// Dim panel border color.
    pub dim_border: Color,
}

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

    let mut doc: toml::Table = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Cannot parse {}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(anyhow::anyhow!("Cannot read {}: {e}", path.display())),
    };

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
    doc.insert("theme".into(), Value::Table(section));

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("Cannot create {}: {e}", parent.display()))?;
        }
    }
    let rendered =
        toml::to_string_pretty(&doc).map_err(|e| anyhow::anyhow!("Cannot serialize theme: {e}"))?;
    std::fs::write(path, rendered)
        .map_err(|e| anyhow::anyhow!("Cannot write {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_colors_case_insensitively() {
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("CYAN"), Some(Color::Cyan));
        assert_eq!(parse_color("  LightBlue "), Some(Color::LightBlue));
        assert_eq!(parse_color("grey"), Some(Color::Gray));
    }

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_color("#ff8800"), Some(Color::Rgb(255, 136, 0)));
        assert_eq!(parse_color("#000000"), Some(Color::Rgb(0, 0, 0)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_color("notacolor"), None);
        assert_eq!(parse_color("#fff"), None);
        assert_eq!(parse_color("#gggggg"), None);
        assert_eq!(parse_color("#ff88000"), None);
    }

    #[test]
    fn from_config_falls_back_per_field() {
        let cfg = ThemeConfig {
            primary: Some("#123456".into()),
            accent: Some("bogus".into()),
            selection_fg: None,
            dim_border: Some("blue".into()),
        };
        let t = Theme::from_config(&cfg);
        assert_eq!(t.primary, Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(t.accent, Theme::default().accent); // bogus → fallback
        assert_eq!(t.selection_fg, Theme::default().selection_fg); // none → fallback
        assert_eq!(t.dim_border, Color::Blue);
    }

    #[test]
    fn default_primary_is_cyan() {
        assert_eq!(Theme::default().primary, Color::Cyan);
    }

    #[test]
    fn cycle_role_walks_palette_and_wraps() {
        let mut t = Theme::default();
        assert_eq!(t.role(0), Color::Cyan); // PALETTE[0]
        t.cycle_role(0, true);
        assert_eq!(t.role(0), Color::LightCyan); // PALETTE[1]
        t.cycle_role(0, false);
        assert_eq!(t.role(0), Color::Cyan);
        t.cycle_role(0, false);
        assert_eq!(t.role(0), PALETTE[PALETTE.len() - 1]); // wrapped
    }

    #[test]
    fn cycle_role_from_custom_steps_into_palette() {
        let mut t = Theme {
            primary: Color::Rgb(1, 2, 3),
            ..Theme::default()
        };
        t.cycle_role(0, true);
        assert_eq!(t.role(0), PALETTE[0]);
    }

    #[test]
    fn color_to_string_round_trips_named_and_hex() {
        for c in PALETTE {
            assert_eq!(parse_color(&color_to_string(c)), Some(c));
        }
        let rgb = Color::Rgb(0xab, 0xcd, 0xef);
        assert_eq!(color_to_string(rgb), "#abcdef");
        assert_eq!(parse_color("#abcdef"), Some(rgb));
    }

    #[test]
    fn persist_theme_writes_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let theme = Theme {
            primary: Color::Rgb(0x10, 0x20, 0x30),
            accent: Color::Green,
            ..Theme::default()
        };
        persist_theme(&path, &theme).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[theme]"), "section header: {text}");
        assert!(text.contains("primary = \"#102030\""), "primary: {text}");
        assert!(text.contains("accent = \"green\""), "accent: {text}");
    }

    #[test]
    fn persist_theme_preserves_unrelated_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "stateDir = \"/tmp/state\"\n\n[backend]\nbaseUrl = \"https://example.test\"\n\n[medulla]\nmaxPasses = 8\n",
        )
        .unwrap();

        persist_theme(&path, &Theme::default()).unwrap();

        let reparsed: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            reparsed["backend"]["baseUrl"].as_str(),
            Some("https://example.test")
        );
        assert_eq!(reparsed["medulla"]["maxPasses"].as_integer(), Some(8));
        assert_eq!(reparsed["stateDir"].as_str(), Some("/tmp/state"));
        assert_eq!(reparsed["theme"]["primary"].as_str(), Some("cyan"));
    }
}
