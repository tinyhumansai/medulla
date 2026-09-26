//! Responsive formatting of a device reading into sidebar lines.
//!
//! Kept apart from sampling so every rendering decision — which metrics fit,
//! how narrow is too narrow, what an unavailable reading looks like — is a pure
//! function of an injected [`DeviceSnapshot`] and can be asserted exactly.
//!
//! The layout itself lives in [`crate::ui::gauge`], which the subscription
//! meters below this block in the footer use too: labels flush left, values
//! flush right, fixed-width bars and percentages.

use medulla::config::{AppearanceConfig, ResourceDisplay};

use crate::ui::gauge::{bar_value, percent, Gauge};

use super::super::types::DeviceSnapshot;

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
    device_gauges(config, sample, width)
        .into_iter()
        .take(max_lines)
        .map(|gauge| gauge.render(width))
        .collect()
}

/// The enabled readings as gauges, already degraded to what `width` can hold.
fn device_gauges(config: &AppearanceConfig, sample: DeviceSnapshot, width: usize) -> Vec<Gauge> {
    [
        metric_gauge(
            "Device CPU",
            config.device_cpu,
            sample.cpu_fraction,
            None,
            width,
        ),
        metric_gauge(
            "Device RAM",
            config.device_ram,
            fraction(sample.memory_used_bytes, sample.memory_total_bytes),
            sample.memory_used_bytes.zip(sample.memory_total_bytes),
            width,
        ),
        metric_gauge(
            "Device disk",
            config.device_disk,
            fraction(sample.disk_used_bytes, sample.disk_total_bytes),
            sample.disk_used_bytes.zip(sample.disk_total_bytes),
            width,
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Render one labelled metric, or `None` when it is switched off.
///
/// An enabled metric whose reading is missing still emits a line: a metric that
/// silently vanished would read as "nothing to report" rather than "this
/// platform cannot say".
fn metric_gauge(
    label: &str,
    display: ResourceDisplay,
    fraction: Option<f64>,
    values: Option<(u64, u64)>,
    width: usize,
) -> Option<Gauge> {
    if display == ResourceDisplay::Off {
        return None;
    }
    let Some(fraction) = fraction.map(|value| value.clamp(0.0, 1.0)) else {
        return Some(Gauge::new(label, UNAVAILABLE));
    };
    let percent = Gauge::new(label, percent(fraction));
    let gauge = match display {
        ResourceDisplay::Off => return None,
        ResourceDisplay::Percent => percent,
        // `Value` and `Bar` both need room. When the finished line does not fit,
        // or the byte counts behind a `Value` are unavailable, the percentage is
        // the detail worth keeping. Measuring the assembled gauge beats a fixed
        // minimum width: a byte pair's length depends on the magnitudes in it,
        // so any constant is simultaneously too strict for `8G/32G` and too
        // generous for a petabyte filesystem.
        ResourceDisplay::Value => match values {
            Some((used, total)) => {
                let pair = Gauge::new(label, format!("{}/{}", bytes(used), bytes(total)));
                if pair.natural_width() <= width {
                    pair
                } else {
                    percent
                }
            }
            None => percent,
        },
        ResourceDisplay::Bar => {
            let bar = Gauge::new(label, bar_value(fraction));
            if bar.natural_width() <= width {
                bar
            } else {
                percent
            }
        }
    };
    Some(gauge)
}

/// Used over total as a 0..=1 fraction, or `None` if either side is missing or
/// the total is zero.
fn fraction(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    let (used, total) = used.zip(total)?;
    (total > 0).then(|| used as f64 / total as f64)
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

/// Columns the enabled device readings need to render in their configured form.
///
/// The rail is sized from its rows, but the footer lives in the same rail: with
/// only the orchestrator row present the sidebar came out at 18 columns, so a
/// configured bar (19–20 columns) silently degraded to a percentage and the
/// `Bar` setting looked broken until unrelated rows widened the rail.
///
/// Percentages and bars are fixed width, so they measure the same whatever the
/// machine is doing. A byte pair is not: it is widest at whatever magnitudes the
/// host actually reports, and widest again when used catches up with total, so
/// the hint is taken over both the live reading and a saturated copy of it.
pub fn device_width_hint(config: &AppearanceConfig, sample: DeviceSnapshot) -> usize {
    let saturated = DeviceSnapshot {
        cpu_fraction: Some(1.0),
        // Used equal to total reads as 100% for the bar and percentage forms.
        memory_used_bytes: sample.memory_total_bytes,
        disk_used_bytes: sample.disk_total_bytes,
        ..sample
    };
    [sample, saturated]
        .into_iter()
        // Measured against an unbounded rail: the question is what the readings
        // *want*, and asking at a real width would degrade them first and then
        // report the narrower answer as the requirement.
        .flat_map(|reading| device_gauges(config, reading, usize::MAX))
        .map(|gauge| gauge.natural_width())
        .max()
        .unwrap_or(0)
}
