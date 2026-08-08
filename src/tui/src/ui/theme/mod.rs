//! The TUI color theme: a small set of roles that drive selection highlighting,
//! panel chrome, and accents. Defaults to the terminal's primary red.
//!
//! Colors come from the optional `[theme]` config section (named ratatui colors
//! or `#rrggbb` hex), with per-field fallback to the defaults. The Appearance
//! subpage edits the live theme and persists just the `[theme]` keys back into
//! the user-global config via [`persist_theme`].

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};

use medulla::config::ThemeConfig;

/// The editable theme roles, in Appearance-editor order.
pub const THEME_ROLES: [&str; 5] = [
    "Primary",
    "Accent",
    "Selection text",
    "Dim border",
    "Attention",
];

/// A curated palette the Appearance editor cycles through. The default color
/// comes first so a first edit can be reversed; named colors keep the persisted
/// values readable.
pub const PALETTE: [Color; 10] = [
    Color::Red,
    Color::Cyan,
    Color::LightCyan,
    Color::Blue,
    Color::LightBlue,
    Color::Magenta,
    Color::Green,
    Color::Yellow,
    Color::White,
    Color::DarkGray,
];

/// Milliseconds between event-loop ticks, and so between animation frames.
///
/// Every frame-driven effect in the TUI counts in this unit, which is why it is
/// declared beside the theme rather than inside the loop that advances it: a
/// pulse configured in seconds has to be converted into frames somewhere, and
/// that conversion must use the same number the loop actually ticks at.
pub const FRAME_MS: u64 = 90;

/// The shortest attention pulse an operator may configure.
///
/// Below about a fifth of a second a pulse stops reading as a rhythm and starts
/// reading as a rendering fault — and at two frames per phase there is nothing
/// left to slow down.
const MIN_BLINK_MS: u64 = 200;

/// The longest attention pulse an operator may configure.
///
/// A cue that spends fifteen seconds dim is a cue nobody notices, which is the
/// same as no cue at all. Ten seconds is generous and still visibly alive.
const MAX_BLINK_MS: u64 = 10_000;

/// The default attention pulse: one second, so a waiting harness reads as a
/// heartbeat rather than as a strobe.
const DEFAULT_BLINK_MS: u64 = 1_000;

impl Default for Theme {
    fn default() -> Self {
        Theme {
            primary: Color::Red,
            accent: Color::Magenta,
            selection_fg: Color::White,
            dim_border: Color::DarkGray,
            attention: Color::Yellow,
            attention_blink: true,
            attention_blink_ms: DEFAULT_BLINK_MS,
        }
    }
}

/// Convert a configured pulse length in seconds into whole milliseconds.
///
/// Clamped rather than rejected: a config that asks for a 0.001-second blink has
/// asked for something, and the nearest thing we can draw is better than
/// silently falling back to the default and leaving the operator to wonder why
/// their key did nothing. Non-finite values have asked for nothing at all and
/// take the default.
pub fn blink_ms_from_seconds(seconds: f64) -> u64 {
    if !seconds.is_finite() {
        return DEFAULT_BLINK_MS;
    }
    let ms = (seconds * 1_000.0).round();
    clamp_blink_ms(ms as u64)
}

/// Clamp an already-millisecond pulse duration to the supported range.
pub fn clamp_blink_ms(ms: u64) -> u64 {
    ms.clamp(MIN_BLINK_MS, MAX_BLINK_MS)
}

/// Render a pulse length back into the seconds the config is written in.
pub fn blink_seconds(ms: u64) -> f64 {
    (ms as f64) / 1_000.0
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
            attention: pick(&cfg.attention, d.attention),
            attention_blink: cfg.attention_blink.unwrap_or(d.attention_blink),
            attention_blink_ms: cfg
                .attention_blink_seconds
                .map(blink_ms_from_seconds)
                .unwrap_or(d.attention_blink_ms),
        }
    }

    /// Whether the attention pulse is in its bright phase on `frame`.
    ///
    /// The pulse is driven off the event loop's frame counter rather than
    /// `Modifier::SLOW_BLINK`, and that is the whole point of it existing. The
    /// modifier delegates the decision to the terminal, and most terminals —
    /// every one built on the common emulator cores, and every multiplexer in
    /// front of them — drop it on the floor. A cue that blinks on one operator's
    /// machine and sits still on another's is not a cue. Counting frames
    /// ourselves means the same rhythm everywhere, at the rate the operator
    /// asked for.
    ///
    /// A pulse with blinking switched off is permanently bright, so callers can
    /// use this unconditionally.
    pub fn attention_bright(&self, frame: usize) -> bool {
        if !self.attention_blink {
            return true;
        }
        // Half a cycle per phase, and never less than one frame: a period below
        // the tick rate would otherwise divide to zero and alternate on every
        // frame regardless of what was configured.
        let half = ((self.attention_blink_ms / 2 + FRAME_MS / 2) / FRAME_MS).max(1) as usize;
        (frame / half).is_multiple_of(2)
    }

    /// The style for a cue pulsing in `color` on `frame`.
    ///
    /// Bright is bold and dim is dimmed, rather than bright and *absent*: text
    /// that vanishes for half a second is text an operator has to wait to read,
    /// and the rail's cues carry the wording that says what the harness wants.
    /// The pulse is meant to catch an eye moving past, not to hide the answer
    /// once it arrives.
    pub fn pulse(&self, color: Color, frame: usize) -> Style {
        let base = Style::default().fg(color);
        if self.attention_bright(frame) {
            base.add_modifier(Modifier::BOLD)
        } else {
            base.add_modifier(Modifier::DIM)
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
            3 => self.dim_border,
            _ => self.attention,
        }
    }

    fn set_role(&mut self, index: usize, color: Color) {
        match index {
            0 => self.primary = color,
            1 => self.accent = color,
            2 => self.selection_fg = color,
            3 => self.dim_border = color,
            _ => self.attention = color,
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
    section.insert(
        "attention".into(),
        Value::String(color_to_string(theme.attention)),
    );
    section.insert(
        "attentionBlink".into(),
        Value::Boolean(theme.attention_blink),
    );
    section.insert(
        "attentionBlinkSeconds".into(),
        Value::Float(blink_seconds(theme.attention_blink_ms)),
    );
    medulla::config::persist_section(path, "theme", section)
}

#[cfg(test)]
mod tests;

mod types;
pub use types::Theme;
