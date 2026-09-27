//! Privacy-conscious product analytics sent to Medulla's OpenPanel project.
//!
//! Records only lifecycle events, normalized screen/interaction names,
//! aggregate token counts, and the authenticated account id: prompts,
//! workspace paths, configuration, and command arguments never enter this
//! boundary. The account id comes from the same slot crash reports use
//! ([`crate::observability::set_user`]), so the two can never disagree about
//! who is signed in, and [`crate::observability::DISABLED_ENV`]
//! (`MEDULLA_ANALYTICS_DISABLED`) switches both off.
//!
//! # Wire format
//!
//! Payloads and headers match the `openpanel_rust` SDK (see [`payload`]'s
//! docs), but this module posts them itself over the workspace's reqwest 0.12
//! with rustls and `ring`. The SDK cannot be used as a dependency here: its
//! only constructor, `Tracker::try_new_from_env`, requires a `.env` file
//! (`dotenvy::dotenv()?` fails without one) and reads its configuration from
//! the process environment, so it cannot run on a user's machine.
//!
//! # Configuration
//!
//! The Medulla project's client id and ingestion URL are compiled-in constants
//! with no build-time or run-time overrides. A native client has no browser
//! `Origin`, so the Medulla OpenPanel client is configured server-side to
//! ignore CORS and secret checks (`ignoreCorsAndSecret`): ingestion needs only
//! the public client id, and no secret is ever compiled in or sent. Analytics
//! is therefore active in every build unless [`DISABLED_ENV`] opts the process
//! out.
//!
//! This crate's own unit tests never resolve the process-wide tracker to an
//! active one, so a test run cannot post events to the live project; the
//! transport is exercised against a local listener instead.

mod config;
mod payload;
mod types;

#[cfg(test)]
mod tests;

use std::sync::OnceLock;
use std::time::Duration;

pub use crate::observability::DISABLED_ENV;
pub use types::{AnalyticsError, AnalyticsStatus};

use config::OpenPanelConfig;
use payload::Payload;

/// Per-request ceiling: analytics must never hold up a sign-in or an exit.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Why analytics is or is not active in this process.
pub fn status() -> AnalyticsStatus {
    match tracker() {
        Ok(_) => AnalyticsStatus::Active,
        Err(status) => status,
    }
}

/// Identify the signed-in account and record that the application started.
///
/// Best-effort, like every function here: telemetry must not keep a user out
/// of the app, so callers discard the error.
pub async fn record_application_started(user_id: &str) -> Result<(), AnalyticsError> {
    let tracker = tracker().map_err(AnalyticsError::Inactive)?;
    tracker.deliver(&Payload::identify(user_id)).await?;
    tracker
        .deliver(&Payload::track(
            "application_started",
            Some(user_id),
            [
                ("os", std::env::consts::OS.to_owned()),
                ("architecture", std::env::consts::ARCH.to_owned()),
            ],
        ))
        .await
}

/// Identify an account and record a completed sign-in.
pub async fn record_sign_in(user_id: &str) -> Result<(), AnalyticsError> {
    let tracker = tracker().map_err(AnalyticsError::Inactive)?;
    tracker.deliver(&Payload::identify(user_id)).await?;
    tracker
        .deliver(&Payload::track("signed_in", Some(user_id), []))
        .await
}

/// Record a sign-out. Call before the stored session is removed.
pub async fn record_sign_out(user_id: &str) -> Result<(), AnalyticsError> {
    let tracker = tracker().map_err(AnalyticsError::Inactive)?;
    tracker
        .deliver(&Payload::track("signed_out", Some(user_id), []))
        .await
}

/// Record a top-level TUI screen view for the signed-in account.
///
/// Fire-and-forget: spawns onto the current tokio runtime, and does nothing
/// outside one, without a signed-in account, or while analytics is inactive.
pub fn record_screen_view(screen: &str) {
    spawn_for_current_user("screen_viewed", [("screen", screen.to_owned())]);
}

/// Record a normalized TUI interaction name (never its payload) for the
/// signed-in account. Fire-and-forget, as [`record_screen_view`].
pub fn record_ui_action(action: &str) {
    spawn_for_current_user("ui_action", [("action", action.to_owned())]);
}

/// Record provider-reported token counts for the signed-in account.
///
/// Counts only, parsed by the harness from its own protocol; no prompt or
/// completion text crosses this boundary. Fire-and-forget.
pub fn record_token_usage(input_tokens: i64, output_tokens: i64) {
    spawn_for_current_user(
        "token_usage_reported",
        [
            ("input_tokens", input_tokens.to_string()),
            ("output_tokens", output_tokens.to_string()),
        ],
    );
}

/// Send one `analytics_test` event and return OpenPanel's HTTP status.
///
/// Backs the hidden `medulla analytics-test` command. A response of any status
/// is `Ok` — a `401` is exactly what the diagnostic needs to show — and only
/// an inactive tracker or a request that got no response is an error.
pub async fn send_test_event() -> Result<u16, AnalyticsError> {
    let tracker = tracker().map_err(AnalyticsError::Inactive)?;
    let payload = Payload::track(
        "analytics_test",
        crate::observability::current_user().as_deref(),
        [("diagnostic", "analytics-test".to_owned())],
    );
    tracker.send(&payload).await
}

/// A narrow OpenPanel HTTP client for Medulla's own events.
struct Tracker {
    endpoint: String,
    client: reqwest::Client,
}

impl Tracker {
    /// Build the client for `config`. `None` when a configured value is not a
    /// legal header value or the client cannot be built.
    fn new(config: &OpenPanelConfig) -> Option<Self> {
        let client = reqwest::Client::builder()
            .default_headers(config.headers()?)
            .timeout(REQUEST_TIMEOUT)
            // One shared client outlives any single runtime (tests start one
            // per case); a pooled connection bound to a finished runtime
            // would fail its next request. Events are rare, so skip pooling.
            .pool_max_idle_per_host(0)
            .build()
            .ok()?;
        Some(Self {
            endpoint: config.endpoint(),
            client,
        })
    }

    /// Post `payload` and return the response status, whatever it is.
    async fn send(&self, payload: &Payload) -> Result<u16, AnalyticsError> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(payload)
            .send()
            .await?;
        Ok(response.status().as_u16())
    }

    /// Post `payload`, treating a non-success status as an error so a
    /// rejected event is never mistaken for a delivered one.
    async fn deliver(&self, payload: &Payload) -> Result<(), AnalyticsError> {
        match self.send(payload).await? {
            status if (200..300).contains(&status) => Ok(()),
            status => Err(AnalyticsError::Rejected(status)),
        }
    }
}

/// The process-wide tracker, resolved on first use.
///
/// Resolution reads the opt-out once; analytics has no runtime configuration
/// beyond it. Under this crate's unit tests it always resolves as opted out,
/// so no test can reach the live OpenPanel project.
fn tracker() -> Result<&'static Tracker, AnalyticsStatus> {
    static TRACKER: OnceLock<Result<Tracker, AnalyticsStatus>> = OnceLock::new();
    TRACKER
        .get_or_init(|| {
            config::resolve(cfg!(test) || crate::observability::opted_out())
            .and_then(|config| Tracker::new(&config).ok_or(AnalyticsStatus::InvalidConfig))
        })
        .as_ref()
        .map_err(|status| *status)
}

/// Spawn a best-effort `name` event for the signed-in account, if there is one
/// and a runtime to run it on.
fn spawn_for_current_user<const N: usize>(
    name: &'static str,
    properties: [(&'static str, String); N],
) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(user_id) = crate::observability::current_user() else {
        return;
    };
    let Ok(tracker) = tracker() else {
        return;
    };
    runtime.spawn(async move {
        let _ = tracker
            .deliver(&Payload::track(name, Some(&user_id), properties))
            .await;
    });
}
