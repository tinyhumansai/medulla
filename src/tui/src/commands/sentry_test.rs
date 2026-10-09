//! The hidden `medulla sentry-test` diagnostic.
//!
//! Sends one informational event through the crash-reporting client `main`
//! started and prints its event id, so an operator (or a release check) can
//! confirm a build's Sentry wiring end to end and then find the event in
//! Sentry by id. Deliberately not listed in `help`.

use std::time::Duration;

use medulla::observability::{self, TestEventReport};

/// How long to wait for the event to be delivered before giving up.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Run `medulla sentry-test`.
///
/// Errors when crash reporting is inactive (opted out, bad DSN override) or when
/// the event could not be confirmed as accepted, so the exit status is usable
/// from a script.
pub(crate) async fn run_sentry_test() -> anyhow::Result<()> {
    // Flushing blocks on the transport thread; keep it off the async workers.
    let report = tokio::task::spawn_blocking(|| observability::send_test_event(DELIVERY_TIMEOUT))
        .await?
        .map_err(|status| anyhow::anyhow!("{status}"))?;
    println!("{}", describe(&report));
    match (report.flushed, report.http_status) {
        (true, Some(status)) if (200..300).contains(&status) => Ok(()),
        _ => anyhow::bail!("the test event was not confirmed as accepted by Sentry"),
    }
}

/// Render the outcome for a person reading a terminal.
fn describe(report: &TestEventReport) -> String {
    let delivery = match (report.flushed, report.http_status) {
        (_, Some(status)) => format!("ingestion answered HTTP {status}"),
        (true, None) => "sent, but no response was received".to_string(),
        (false, None) => "not delivered before the timeout".to_string(),
    };
    format!("sentry event id: {}\n{delivery}", report.event_id.simple())
}
