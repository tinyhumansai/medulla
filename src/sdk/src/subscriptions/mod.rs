//! Subscription-usage meters: how much of each paid allowance Medulla runs on
//! has already been spent.
//!
//! The device readings in the Agents sidebar answer "can this machine take
//! another agent". These answer the other question an operator asks before
//! dispatching work — "can my accounts". Both windows an agent can exhaust
//! (Claude's session and weekly limits, Codex's rate-limit windows) and the
//! balances it can drain (OpenRouter credits, the TinyHumans portal) are
//! sampled here into one shape the sidebar and the settings page both render.
//!
//! Three rules hold across every probe:
//!
//! - **Nothing is sampled that is not switched on.** Two of these sources are
//!   network calls against the operator's own subscriptions; with the meters
//!   off, no request leaves the machine.
//! - **A probe never fails the sample.** Each source returns its own result, and
//!   a source that could not answer leaves its meters carrying the reason
//!   instead of removing them. A meter that vanished would read as "nothing to
//!   report", which is a different claim from "could not ask".
//! - **Credentials are borrowed, never asked for.** Claude's token comes from
//!   the local CLI's own login and OpenRouter's key from an environment
//!   variable the config only *names*. No secret is ever written into the
//!   config this TUI rewrites.

mod claude;
mod codex;
mod openrouter;
mod time;
mod tinyhumans;
mod types;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use serde_json::Value;

use crate::config::{MeterId, MeterSource, ResourceDisplay, SubscriptionsConfig};

pub use openrouter::DEFAULT_KEY_ENV as OPENROUTER_KEY_ENV;
pub use types::{MeterReading, SubscriptionSnapshot};

/// Sample every switched-on meter.
///
/// `account_usage` is the TinyHumans usage document the Usage page already
/// loads, passed in rather than re-fetched. Pass `None` when it has not loaded
/// yet: the portal meter then reports itself waiting rather than empty.
///
/// The Claude and OpenRouter probes are network calls and run concurrently; the
/// Codex probe reads a local file and runs on a blocking thread so a slow disk
/// cannot stall the runtime.
pub async fn sample(
    config: &SubscriptionsConfig,
    account_usage: Option<&Value>,
) -> SubscriptionSnapshot {
    let mut meters: BTreeMap<MeterId, MeterReading> = BTreeMap::new();
    if !config.any_enabled() {
        return SubscriptionSnapshot {
            meters,
            sampled_at: now_seconds(),
        };
    }

    // The two network probes run together: they are independent, each has its
    // own timeout, and the sidebar should not wait for their sum.
    let (claude, open_router) = futures::join!(
        async {
            match config.source_enabled(MeterSource::Claude) {
                true => Some(claude::sample().await),
                false => None,
            }
        },
        async {
            match config.source_enabled(MeterSource::OpenRouter) {
                true => Some(openrouter::sample(config.open_router_key_env.as_deref()).await),
                false => None,
            }
        }
    );

    if let Some(result) = claude {
        absorb(&mut meters, config, MeterSource::Claude, result);
    }
    if let Some(result) = open_router {
        let result = result.map(|reading| vec![(MeterId::OpenRouterCredits, reading)]);
        absorb(&mut meters, config, MeterSource::OpenRouter, result);
    }
    if config.source_enabled(MeterSource::Codex) {
        let result = tokio::task::spawn_blocking(codex::sample)
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
        absorb(&mut meters, config, MeterSource::Codex, result);
    }
    if config.source_enabled(MeterSource::Tinyhumans) {
        let result = match account_usage {
            Some(usage) => tinyhumans::reading(usage)
                .map(|reading| vec![(MeterId::TinyhumansBalance, reading)])
                .ok_or_else(|| "no portal balance on this account".to_string()),
            None => Err("account usage not loaded".to_string()),
        };
        absorb(&mut meters, config, MeterSource::Tinyhumans, result);
    }

    SubscriptionSnapshot {
        meters,
        sampled_at: now_seconds(),
    }
}

/// File one source's result under the enabled meters that read from it.
///
/// A probe's failure lands on every switched-on meter of that source, because
/// the operator switched those meters on and each of them is now unanswered. A
/// probe that succeeded but did not mention a meter leaves that meter saying so
/// — a plan without a per-model weekly window is a real answer, not an outage.
fn absorb(
    meters: &mut BTreeMap<MeterId, MeterReading>,
    config: &SubscriptionsConfig,
    source: MeterSource,
    result: Result<Vec<(MeterId, MeterReading)>, String>,
) {
    let enabled = MeterId::ALL
        .into_iter()
        .filter(|meter| meter.source() == source && config.display(*meter) != ResourceDisplay::Off);
    match result {
        Ok(readings) => {
            for meter in enabled {
                let reading = readings
                    .iter()
                    .find(|(id, _)| *id == meter)
                    .map(|(_, reading)| reading.clone())
                    .unwrap_or_else(|| MeterReading::unavailable("not reported for this plan"));
                meters.insert(meter, reading);
            }
        }
        Err(reason) => {
            for meter in enabled {
                meters.insert(meter, MeterReading::unavailable(reason.clone()));
            }
        }
    }
}

/// Wall clock in seconds since the Unix epoch.
fn now_seconds() -> i64 {
    crate::clock::now_millis() / 1000
}
