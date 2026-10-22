//! Precomputed resource gradients and terminal color-depth conversion.

use ratatui::style::Color;

/// A linear, inclusive 101-entry gradient, indexed by percentages 0 through 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gradient {
    /// Color at 0 percent.
    pub start: Color,
    /// Optional midpoint at 50 percent.
    pub mid: Option<Color>,
    /// Optional endpoint at 100 percent. Without one the gradient is solid.
    pub end: Option<Color>,
    colors: [Color; 101],
}

impl Gradient {
    /// Construct a solid, two-color, or three-color gradient.
    pub fn new(start: Color, mid: Option<Color>, end: Option<Color>) -> Self {
        let colors = std::array::from_fn(|pct| {
            let Some(end) = end else { return start };
            match mid {
                Some(mid) if pct <= 50 => interpolate(start, mid, pct, 50),
                Some(mid) => interpolate(mid, end, pct - 50, 50),
                None => interpolate(start, end, pct, 100),
            }
        });
        Self {
            start,
            mid,
            end,
            colors,
        }
    }

    /// Look up a precomputed color. Values above 100 are clamped to the end.
    pub fn at(&self, pct: usize) -> Color {
        self.colors[pct.min(100)]
    }

    /// Convert every control point and precomputed color to a terminal palette.
    pub fn with_depth(self, depth: ColorDepth) -> Self {
        let convert = |color| depth.convert(color);
        Self {
            start: convert(self.start),
            mid: self.mid.map(convert),
            end: self.end.map(convert),
            colors: self.colors.map(convert),
        }
    }
}

fn interpolate(a: Color, b: Color, step: usize, steps: usize) -> Color {
    if step == 0 {
        return a;
    }
    if step == steps {
        return b;
    }
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    let channel = |from: u8, to: u8| {
        (i32::from(from) + (step as i32 * (i32::from(to) - i32::from(from))) / steps as i32) as u8
    };
    Color::Rgb(channel(ar, br), channel(ag, bg), channel(ab, bb))
}

fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 0, 0),
        Color::Green => (0, 205, 0),
        Color::Yellow => (205, 205, 0),
        Color::Blue => (0, 0, 238),
        Color::Magenta => (205, 0, 205),
        Color::Cyan => (0, 205, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (127, 127, 127),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (92, 92, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        Color::Indexed(index @ 0..=15) => rgb(TTY_PALETTE[index as usize]),
        Color::Indexed(index @ 16..=231) => {
            const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let index = index - 16;
            (
                LEVELS[(index / 36) as usize],
                LEVELS[((index / 6) % 6) as usize],
                LEVELS[(index % 6) as usize],
            )
        }
        Color::Indexed(index) => {
            let gray = 8 + 10 * (index.saturating_sub(232));
            (gray, gray, gray)
        }
        Color::Reset => (255, 255, 255),
    }
}

/// Terminal palette supported for resource colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24-bit RGB.
    TrueColor,
    /// The xterm 256-color palette.
    Color256,
    /// The ANSI 16-color palette.
    Color16,
}

impl ColorDepth {
    /// Detect common terminal capabilities from `COLORTERM` and `TERM`.
    pub fn detect() -> Self {
        Self::detect_from(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    }

    /// Detect terminal capabilities from explicit environment values.
    pub fn detect_from(colorterm: Option<&str>, term: Option<&str>) -> Self {
        if colorterm.is_some_and(|value| {
            matches!(value.to_ascii_lowercase().as_str(), "truecolor" | "24bit")
        }) {
            Self::TrueColor
        } else if term.is_some_and(|value| value.to_ascii_lowercase().contains("256color")) {
            Self::Color256
        } else {
            Self::Color16
        }
    }

    /// Map an RGB color to the selected terminal palette.
    pub fn convert(self, color: Color) -> Color {
        match (self, color) {
            (Self::TrueColor, color) => color,
            (Self::Color256, Color::Rgb(r, g, b)) => truecolor_to_256(Color::Rgb(r, g, b)),
            (Self::Color16, Color::Rgb(r, g, b)) => truecolor_to_16(Color::Rgb(r, g, b)),
            (_, color) => color,
        }
    }
}

/// Convert RGB to btop's xterm 256-color cube or grayscale ramp.
pub fn truecolor_to_256(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let gray = (f64::from(r) / 11.0).round() as u8;
    if gray == (f64::from(g) / 11.0).round() as u8 && gray == (f64::from(b) / 11.0).round() as u8 {
        Color::Indexed(232 + gray)
    } else {
        let cube = |channel: u8| (f64::from(channel) / 51.0).round() as u8;
        Color::Indexed(16 + cube(r) * 36 + cube(g) * 6 + cube(b))
    }
}

/// The conventional ANSI palette, with normal colors first and bright colors second.
pub const TTY_PALETTE: [Color; 16] = [
    Color::Rgb(0, 0, 0),
    Color::Rgb(205, 0, 0),
    Color::Rgb(0, 205, 0),
    Color::Rgb(205, 205, 0),
    Color::Rgb(0, 0, 238),
    Color::Rgb(205, 0, 205),
    Color::Rgb(0, 205, 205),
    Color::Rgb(229, 229, 229),
    Color::Rgb(127, 127, 127),
    Color::Rgb(255, 0, 0),
    Color::Rgb(0, 255, 0),
    Color::Rgb(255, 255, 0),
    Color::Rgb(92, 92, 255),
    Color::Rgb(255, 0, 255),
    Color::Rgb(0, 255, 255),
    Color::Rgb(255, 255, 255),
];

/// Convert RGB to the nearest entry in the conventional 16-color TTY palette.
pub fn truecolor_to_16(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let nearest = TTY_PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| {
            let Color::Rgb(cr, cg, cb) = candidate else {
                unreachable!()
            };
            let dr = i32::from(r) - i32::from(*cr);
            let dg = i32::from(g) - i32::from(*cg);
            let db = i32::from(b) - i32::from(*cb);
            dr * dr + dg * dg + db * db
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ][nearest]
}
