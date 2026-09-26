//! Configuration for the subscription-usage meters shown in the Agents sidebar.
//!
//! Kept beside `appearance` rather than inside it: these are readings about the
//! *accounts* Medulla spends against, not about this machine, and they are
//! sampled from providers rather than from the kernel.

mod types;

pub use types::{
    MeterId, MeterSource, SubscriptionsConfig,
    DEFAULT_REFRESH_SECONDS as DEFAULT_SUBSCRIPTION_REFRESH_SECONDS,
    MIN_REFRESH_SECONDS as MIN_SUBSCRIPTION_REFRESH_SECONDS,
};
