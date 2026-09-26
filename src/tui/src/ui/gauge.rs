//! The shared shape of a sidebar footer reading: a label on the left, a value
//! flushed to the right edge, and fixed-width numbers between them.
//!
//! The device readings and the subscription meters share one footer, so they
//! have to share one layout — two blocks that each aligned to themselves would
//! read as two lists that nearly line up, which is worse than either alone.
//! Three rules do the work:
//!
//! - **Values are flushed right.** Labels vary in length and are read by
//!   scanning down the left edge; numbers are compared against each other and
//!   want a column of their own.
//! - **Percentages are fixed width.** `  6%` and `100%` occupy the same four
//!   columns, so a bar does not shuffle sideways as usage moves — and the rail
//!   no longer has to be sized against a saturated reading to stop it.
//! - **Bars are fixed width.** Same glyph count in both blocks, so the filled
//!   portions can be compared at a glance.
//!
//! Assembling a line is therefore two steps: build the [`Gauge`] (which knows
//! its natural width, and is what the rail is sized against) and then render it
//! into whatever width the rail actually turned out to be.

use unicode_width::UnicodeWidthStr;

/// Glyph count of a compact pressure bar.
pub const BAR_WIDTH: usize = 4;

/// Columns a rendered percentage always occupies: three digits and the sign.
pub const PERCENT_WIDTH: usize = 4;

/// Minimum blank columns between a label and its value.
///
/// One, not zero: a label that ran straight into its number would be unreadable
/// on the one rail width where they exactly met.
const MIN_GAP: usize = 1;

/// One footer reading, before it knows how wide the rail is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gauge {
    /// What the reading is, shown flush left.
    pub label: String,
    /// The reading itself, shown flush right.
    pub value: String,
}

impl Gauge {
    /// A reading of `value` labelled `label`.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }

    /// The narrowest rail this reading fits on without crowding.
    ///
    /// What the rail's width hint is taken from: a rail sized to this shows the
    /// reading in the form it was configured in, and a rail narrower than it is
    /// what makes the caller degrade to a shorter form.
    pub fn natural_width(&self) -> usize {
        self.label.width() + MIN_GAP + self.value.width()
    }

    /// Render into `width` columns with the value flushed to the right edge.
    ///
    /// On a rail too narrow for the aligned form, the value gives up its own
    /// padding first — a fixed-width `  6%` becomes `6%` — because alignment is
    /// worth nothing on a rail with no spare columns to align *in*, while the
    /// digits are worth everything. A reading still too wide after that is
    /// emitted at its natural width rather than truncated: the caller degrades
    /// to a shorter form before it gets here, and silently cutting a number in
    /// half would be the one outcome worse than an overhanging one.
    pub fn render(&self, width: usize) -> String {
        let mut value = self.value.as_str();
        if self.label.width() + MIN_GAP + value.width() > width {
            value = value.trim_start();
        }
        let gap = width
            .saturating_sub(self.label.width() + value.width())
            .max(MIN_GAP);
        format!("{}{}{}", self.label, " ".repeat(gap), value)
    }
}

/// A fixed-width filled/empty block bar for a 0..=1 fraction, with its
/// percentage beside it.
///
/// The two travel together because a bar alone cannot be read precisely and a
/// percentage alone cannot be scanned; the width is constant so neither moves.
pub fn bar_value(fraction: f64) -> String {
    format!("{} {}", bar(fraction), percent(fraction))
}

/// A fixed-width filled/empty block bar for a 0..=1 fraction.
pub fn bar(fraction: f64) -> String {
    let filled = (fraction.clamp(0.0, 1.0) * BAR_WIDTH as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled))
}

/// A 0..=1 fraction as a right-aligned percentage of fixed width.
pub fn percent(fraction: f64) -> String {
    let digits = PERCENT_WIDTH - 1;
    format!(
        "{:>digits$}%",
        (fraction.clamp(0.0, 1.0) * 100.0).round() as u64
    )
}
