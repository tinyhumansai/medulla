//! The hidden `medulla analytics-test` diagnostic.
//!
//! Sends one `analytics_test` event to OpenPanel and prints the HTTP status the
//! ingestion endpoint answered with, so an operator (or a release check) can
//! confirm a build's analytics wiring — the client id, the endpoint, and the
//! project's server-side acceptance of a secretless native client — end to end. Deliberately not listed in `help`.

/// Run `medulla analytics-test`.
///
/// Errors when analytics is inactive (opted out through
/// `MEDULLA_ANALYTICS_DISABLED`), when the request got no response, or when
/// OpenPanel did not accept the event, so the exit status is usable from a
/// script.
pub(crate) async fn run_analytics_test() -> anyhow::Result<()> {
    let status = medulla::analytics::send_test_event()
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    println!("openpanel ingestion answered HTTP {status}");
    if (200..300).contains(&status) {
        Ok(())
    } else {
        anyhow::bail!("the test event was not accepted by OpenPanel")
    }
}
