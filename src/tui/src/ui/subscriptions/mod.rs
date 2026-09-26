//! Rendering of subscription-usage readings into sidebar lines.
//!
//! Sampling lives in the SDK ([`medulla::subscriptions`]); this module owns only
//! the presentation, so every decision about what fits a narrow rail is a pure
//! function of an injected snapshot and can be asserted exactly — the same split
//! the device readings use.

mod format;

#[cfg(test)]
mod tests;

pub use format::{subscription_lines, subscription_width_hint};
