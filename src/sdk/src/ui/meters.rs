//! Compact single-line meters: a labelled bar plus its numbers, sized to fit a
//! header row rather than a dashboard.
//!
//! Every fleet reading the UI shows is a fraction of something declared — memory
//! against the machine's total, load against its cores, a context window against
//! its ceiling, a budget window against its limit. A number alone makes you do
//! the division; a full-width gauge costs a row each. A short bar does the
//! division at a glance and three of them still fit on one line.
//!
//! Colour is by pressure, not by kind: green under two thirds, yellow past it,
//! red past 90%, because the reason to look at these at all is to find the one
//! that is about to stop the work.

use crate::ui::agents::Line;

/// The bar's width in cells. Eight is enough to read a fraction to the nearest
/// eighth and short enough that three meters and their labels fit in 80 columns.
const CELLS: usize = 8;

/// A drawn bar for `fraction` (clamped to 0..=1), using eighth-block glyphs so a
/// partial cell still reads as movement rather than rounding to nothing.
pub fn bar(fraction: f64) -> String {
    let fraction = fraction.clamp(0.0, 1.0);
    let filled = fraction * CELLS as f64;
    let whole = filled.floor() as usize;
    let remainder = filled - whole as f64;
    let mut out = "█".repeat(whole.min(CELLS));
    if whole < CELLS {
        // Eighth blocks, indexed by the remaining fraction of one cell.
        const PARTIAL: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
        let index = (remainder * 8.0).round() as usize;
        out.push_str(PARTIAL[index.min(7)]);
        let drawn = whole + usize::from(index > 0);
        out.push_str(&"░".repeat(CELLS.saturating_sub(drawn)));
    }
    out
}

/// The colour a fraction should read in: the point of these meters is spotting
/// the one that is about to run out.
pub fn pressure_color(fraction: f64) -> &'static str {
    match fraction {
        f if f >= 0.9 => "red",
        f if f >= 0.66 => "yellow",
        _ => "green",
    }
}

/// One meter as `label ▉▉▉░░░░░ detail`, coloured by pressure.
pub fn meter(label: &str, fraction: f64, detail: impl Into<String>) -> Line {
    let detail = detail.into();
    let text = if detail.is_empty() {
        format!("{label} {}", bar(fraction))
    } else {
        format!("{label} {} {detail}", bar(fraction))
    };
    Line {
        text,
        color: Some(pressure_color(fraction).into()),
        dim: false,
    }
}

/// A byte count as a compact magnitude: `512MB`, `12.0GB`.
pub fn bytes(value: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = value as f64;
    if value >= GB {
        format!("{:.1}GB", value / GB)
    } else {
        format!("{:.0}MB", (value / MB).max(0.0))
    }
}

/// A token magnitude: `980`, `12k`, `1M`, `1.2M`.
///
/// A round million prints as `1M`, not `1.0M`. The context window is the number
/// this is most often asked to render, it is almost always exactly round, and a
/// decimal point on a figure with nothing after it reads as spurious precision.
pub fn tokens(value: i64) -> String {
    let value = value.max(0);
    if value < 1_000 {
        return value.to_string();
    }
    if value < 1_000_000 {
        return format!("{}k", (value as f64 / 1_000.0).round() as i64);
    }
    let millions = value as f64 / 1_000_000.0;
    if (millions - millions.round()).abs() < f64::EPSILON {
        format!("{}M", millions.round() as i64)
    } else {
        format!("{millions:.1}M")
    }
}

/// The host's memory meter, or `None` when the host declared no memory numbers.
///
/// Reported as *used* rather than free: pressure is what the colour encodes, and
/// a bar that empties as a machine fills up would invert that.
pub fn memory_meter(resources: &crate::runtime::HostResources) -> Option<Line> {
    let total = resources.total_memory_bytes.filter(|t| *t > 0)?;
    let available = resources.available_memory_bytes?;
    let used = total.saturating_sub(available);
    Some(meter(
        "ram",
        used as f64 / total as f64,
        format!("{} / {}", bytes(used), bytes(total)),
    ))
}

/// The host's CPU meter from its declared load average, or a plain core count
/// when no load was reported.
///
/// Load is per-core-normalised, so 1.0 means "as many runnable threads as
/// cores". Without a load reading there is nothing to fill a bar with — cores
/// are capacity, not usage — so the row states the capacity instead of drawing a
/// meter that would have to invent a number.
pub fn cpu_meter(resources: &crate::runtime::HostResources) -> Option<Line> {
    let cores = resources.cpu_cores.filter(|c| *c > 0.0);
    match (resources.load_average_1m, cores) {
        (Some(load), Some(cores)) => Some(meter(
            "cpu",
            load / cores,
            format!("{load:.2} load / {} cores", cores as i64),
        )),
        (None, Some(cores)) => Some(Line {
            text: format!("cpu · {} cores", cores as i64),
            color: None,
            dim: true,
        }),
        _ => None,
    }
}

/// The disk meter, or `None` when the host declared no free-space reading.
///
/// Free space alone has no denominator to divide by, so this states the figure
/// rather than drawing a fraction it cannot compute.
pub fn disk_line(resources: &crate::runtime::HostResources) -> Option<Line> {
    resources.disk_free_bytes.map(|free| Line {
        text: format!("disk · {} free", bytes(free)),
        color: None,
        dim: true,
    })
}

/// The context meter for one lane: the prompt against the window, the window's
/// size, and the `(in / out / cached)` breakdown.
///
/// The bar is input-against-window because that is the ceiling that actually
/// stops a turn. The window is named rather than left implicit — a bar at 4%
/// means nothing without knowing 4% of what, and the difference between a 32k
/// and a 1M window is the difference between "nearly out of room" and "barely
/// started". Output sits in the breakdown rather than the bar because it does
/// not consume the same budget.
///
/// `cached` is a dash when the provider reported no cache figure. Omitting the
/// field entirely would make the row change shape between providers; inventing a
/// zero would claim a cache miss we never observed.
pub fn context_meter(usage: &LaneUsage, window: i64) -> Option<Line> {
    if usage.input == 0 && usage.output == 0 {
        return None;
    }
    let window = window.max(1);
    let fraction = usage.input as f64 / window as f64;
    let cached = match usage.cache_hit_rate() {
        Some(rate) => format!("{}%", (rate * 100.0).round() as i64),
        None => "—".to_string(),
    };
    let detail = format!(
        "{} window · (in {} / out {} / cached {cached})",
        tokens(window),
        tokens(usage.input),
        tokens(usage.output),
    );
    Some(meter("ctx", fraction, detail))
}

/// A lane's accumulated token usage.
///
/// Latest-wins for the prompt, running total for output: the prompt is the
/// current context size (each turn resends it, so summing would multiply-count
/// the same tokens), while output and cache genuinely accumulate over the lane's
/// life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneUsage {
    /// Prompt tokens on the most recent reported turn.
    pub input: i64,
    /// Output tokens summed across the lane.
    pub output: i64,
    /// Cache-read tokens on the most recent reported turn, when reported.
    pub cache_read: Option<i64>,
}

impl LaneUsage {
    /// Fold one reported usage into the running totals.
    pub fn accumulate(&mut self, usage: &crate::ui::events::Usage) {
        self.input = usage.input_tokens;
        self.output += usage.output_tokens;
        if usage.cache_read_tokens.is_some() {
            self.cache_read = usage.cache_read_tokens;
        }
    }

    /// The share of the current prompt served from cache, when known.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let cached = self.cache_read? as f64;
        (self.input > 0).then(|| (cached / self.input as f64).clamp(0.0, 1.0))
    }
}
