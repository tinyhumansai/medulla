//! The readings the subscription probes produce.

use std::collections::BTreeMap;

use crate::config::MeterId;

/// One meter's current reading.
///
/// Every quantity is optional because every source is partial: a provider that
/// reports a percentage need not report an amount, an account with no cap has
/// no fraction, and a probe that could not reach its source has neither. A
/// reading with nothing in it but a `note` is how "switched on, but not
/// answering" is expressed — the alternative, dropping the meter, reads as
/// "nothing to report", which is a different and wrong claim.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeterReading {
    /// Share of the window or cap already used, 0..=1.
    pub used_fraction: Option<f64>,
    /// Money left to spend, in `currency`.
    pub remaining: Option<f64>,
    /// The cap `remaining` is measured against, when the provider states one.
    pub limit: Option<f64>,
    /// ISO 4217 code for `remaining` and `limit`. `USD` unless a source says
    /// otherwise.
    pub currency: Option<String>,
    /// When the window rolls over, in seconds since the Unix epoch.
    pub resets_at: Option<i64>,
    /// A label refinement the source supplied — the model a scoped window
    /// covers, say — appended to the meter's own label.
    pub qualifier: Option<String>,
    /// Why the reading is empty, when it is. Rendered in the settings page so a
    /// meter that says nothing can be told from one that cannot.
    pub note: Option<String>,
}

impl MeterReading {
    /// A reading that failed, carrying the reason.
    pub fn unavailable(note: impl Into<String>) -> Self {
        Self {
            note: Some(note.into()),
            ..Self::default()
        }
    }

    /// Whether this reading carries a number worth rendering.
    pub fn has_value(&self) -> bool {
        self.used_fraction.is_some() || self.remaining.is_some()
    }
}

/// Every meter's latest reading, and when they were taken.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubscriptionSnapshot {
    /// Readings by meter. A meter absent from the map was never sampled —
    /// switched off, or its source's probe was not run.
    pub meters: BTreeMap<MeterId, MeterReading>,
    /// When this snapshot was taken, in seconds since the Unix epoch. Zero on a
    /// snapshot that has never been sampled.
    pub sampled_at: i64,
}

impl SubscriptionSnapshot {
    /// The reading for `meter`, if it was sampled.
    pub fn get(&self, meter: MeterId) -> Option<&MeterReading> {
        self.meters.get(&meter)
    }

    /// Whether `now` is at least `refresh_seconds` past the last sample.
    ///
    /// A never-sampled snapshot is always stale, which is what makes the first
    /// tick after start-up fetch.
    pub fn is_stale(&self, now: i64, refresh_seconds: u64) -> bool {
        self.sampled_at == 0
            || u64::try_from(now.saturating_sub(self.sampled_at)).unwrap_or_default()
                >= refresh_seconds
    }
}
