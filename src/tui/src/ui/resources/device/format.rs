//! Responsive formatting of a device reading into sidebar lines.
//!
//! Kept apart from sampling so every rendering decision — which metrics fit,
//! how narrow is too narrow, what an unavailable reading looks like — is a pure
//! function of an injected [`DeviceSnapshot`] and can be asserted exactly.

use medulla::config::{AppearanceConfig, ResourceDisplay};

use super::super::types::DeviceSnapshot;

/// Below this inner width the bar glyphs crowd out the label, and the line
/// falls back to a percentage.
const BAR_MIN_WIDTH: usize = 20;

/// Glyph count of a compact pressure bar.
const BAR_WIDTH: usize = 4;

/// Shown in place of a reading the platform could not supply.
const UNAVAILABLE: &str = "—";

/// Format the enabled device readings, highest priority first.
///
/// Ordering is CPU, then RAM, then disk: CPU and RAM change minute to minute
/// and explain a slow run, while disk capacity moves slowly. `max_lines` is the
/// budget the sidebar can spare without eating navigation rows, so the caller
/// shrinks it rather than pushing rows off-screen; `width` is the sidebar's
/// inner width, and narrow rails degrade values and bars to percentages instead
/// of overflowing.
///
/// Metrics set to [`ResourceDisplay::Off`] produce no line at all, so each one
/// is independently switchable from the `[appearance]` config.
pub fn device_lines(
    config: &AppearanceConfig,
    sample: DeviceSnapshot,
    width: usize,
    max_lines: usize,
) -> Vec<String> {
    [
        metric_line(
            "Device CPU",
            config.device_cpu,
            sample.cpu_fraction,
            None,
            width,
        ),
        metric_line(
            "Device RAM",
            config.device_ram,
            fraction(sample.memory_used_bytes, sample.memory_total_bytes),
            sample.memory_used_bytes.zip(sample.memory_total_bytes),
            width,
        ),
        metric_line(
            "Device disk",
            config.device_disk,
            fraction(sample.disk_used_bytes, sample.disk_total_bytes),
            sample.disk_used_bytes.zip(sample.disk_total_bytes),
            width,
        ),
    ]
    .into_iter()
    .flatten()
    .take(max_lines)
    .collect()
}

/// Render one labelled metric, or `None` when it is switched off.
///
/// An enabled metric whose reading is missing still emits a line: a metric that
/// silently vanished would read as "nothing to report" rather than "this
/// platform cannot say".
fn metric_line(
    label: &str,
    display: ResourceDisplay,
    fraction: Option<f64>,
    values: Option<(u64, u64)>,
    width: usize,
) -> Option<String> {
    if display == ResourceDisplay::Off {
        return None;
    }
    let Some(fraction) = fraction.map(|value| value.clamp(0.0, 1.0)) else {
        return Some(format!("{label} {UNAVAILABLE}"));
    };
    let percent = format!("{label} {:.0}%", fraction * 100.0);
    let line = match display {
        ResourceDisplay::Off => return None,
        ResourceDisplay::Percent => percent,
        // `Value` and `Bar` both need room. When the finished line does not fit,
        // or the byte counts behind a `Value` are unavailable, the percentage is
        // the detail worth keeping. Measuring the assembled line beats a fixed
        // minimum width: a byte pair's length depends on the magnitudes in it,
        // so any constant is simultaneously too strict for `8G/32G` and too
        // generous for a petabyte filesystem. Labels and byte counts are ASCII,
        // so bytes are columns here.
        ResourceDisplay::Value => match values {
            Some((used, total)) => {
                let pair = format!("{label} {}/{}", bytes(used), bytes(total));
                if pair.len() <= width {
                    pair
                } else {
                    percent
                }
            }
            None => percent,
        },
        ResourceDisplay::Bar if width >= BAR_MIN_WIDTH => {
            let bar = format!("{label} {} {:.0}%", bar(fraction), fraction * 100.0);
            if bar.chars().count() <= width {
                bar
            } else {
                percent
            }
        }
        // `Value` is handled above for every width now that it measures its own
        // line, so only a bar too narrow for its glyphs still lands here.
        ResourceDisplay::Bar => percent,
    };
    Some(line)
}

/// Used over total as a 0..=1 fraction, or `None` if either side is missing or
/// the total is zero.
fn fraction(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    let (used, total) = used.zip(total)?;
    (total > 0).then(|| used as f64 / total as f64)
}

/// A fixed-width filled/empty block bar for a 0..=1 fraction.
fn bar(fraction: f64) -> String {
    let filled = (fraction.clamp(0.0, 1.0) * BAR_WIDTH as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled))
}

/// Byte counts at device scale: whole gigabytes, dropping to megabytes for the
/// small filesystems (containers, ramdisks) where "0G" would say nothing.
fn bytes(value: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = value as f64;
    if value >= GIB {
        format!("{:.0}G", value / GIB)
    } else {
        format!("{:.0}M", value / MIB)
    }
}
