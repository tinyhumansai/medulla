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
//! The Medulla project's client id is compiled in. The API URL can be
//! overridden with [`API_URL_ENV`] for local diagnostics. A native client has no browser
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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

pub use crate::observability::DISABLED_ENV;
pub use config::API_URL_ENV;
pub use types::{AnalyticsError, AnalyticsStatus};

use config::OpenPanelConfig;
use payload::Payload;

/// Per-request ceiling: analytics must never hold up a sign-in or an exit.
/// Public so a caller awaiting a multi-request call (e.g. [`record_sign_in`])
/// can bound the whole thing by one deadline rather than stacking each
/// request's own timeout.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

static PENDING_EVENTS: AtomicUsize = AtomicUsize::new(0);
static PENDING_EVENTS_CHANGED: OnceLock<tokio::sync::Notify> = OnceLock::new();

/// Wait briefly for previously queued fire-and-forget analytics events.
/// Wrapper commands call this after their final transcript drain and before
/// `process::exit`, which would otherwise abort Tokio tasks immediately.
pub async fn flush_pending() {
    let notify = PENDING_EVENTS_CHANGED.get_or_init(tokio::sync::Notify::new);
    let _ = tokio::time::timeout(REQUEST_TIMEOUT, async {
        loop {
            let changed = notify.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if PENDING_EVENTS.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    })
    .await;
}

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
    let user_id = checked_account_id(user_id)?;
    tracker.deliver(&Payload::identify(&user_id)).await?;
    tracker
        .deliver(&Payload::track(
            "application_started",
            Some(&user_id),
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
    let user_id = checked_account_id(user_id)?;
    tracker.deliver(&Payload::identify(&user_id)).await?;
    tracker
        .deliver(&Payload::track("signed_in", Some(&user_id), []))
        .await
}

/// Record a sign-out. Call before the stored session is removed.
pub async fn record_sign_out(user_id: &str) -> Result<(), AnalyticsError> {
    let tracker = tracker().map_err(AnalyticsError::Inactive)?;
    let user_id = checked_account_id(user_id)?;
    tracker
        .deliver(&Payload::track("signed_out", Some(&user_id), []))
        .await
}

/// The account id to send as `profileId`, held to the same shape crash
/// reports require ([`crate::observability::set_user`]): an email, token,
/// path, or prompt fragment is refused rather than sent.
fn checked_account_id(user_id: &str) -> Result<String, AnalyticsError> {
    crate::observability::account_id(user_id).ok_or(AnalyticsError::InvalidAccountId)
}

/// Record a top-level TUI screen view for the signed-in account.
///
/// Fire-and-forget: spawns onto the current tokio runtime, and does nothing
/// outside one, without a signed-in account, or while analytics is inactive.
pub fn record_screen_view(screen: &str) {
    if let Some(properties) = screen_view_properties(screen) {
        spawn_for_current_user("screen_viewed", properties);
    }
}

/// The `screen_viewed` properties for `screen`, or `None` for anything but a
/// top-level TUI screen: only the five named screens are ever reported.
fn screen_view_properties(screen: &str) -> Option<[(&'static str, String); 1]> {
    matches!(
        screen,
        "Sessions" | "Workflows" | "Subconscious" | "Feedback" | "Settings"
    )
    .then(|| [("screen", screen.to_owned())])
}

/// Record a normalized TUI interaction name (never its payload) for the
/// signed-in account. Fire-and-forget, as [`record_screen_view`].
pub fn record_ui_action(action: &str) {
    if let Some(properties) = ui_action_properties(action) {
        spawn_for_current_user("ui_action", properties);
    }
}

/// The `ui_action` properties for `action`, or `None` for anything but the
/// one normalized action name that is reported.
fn ui_action_properties(action: &str) -> Option<[(&'static str, String); 1]> {
    (action == "command_dispatched").then(|| [("action", action.to_owned())])
}

/// Record provider-reported token counts for the signed-in account.
///
/// Counts only, parsed by the harness from its own protocol; no prompt or
/// completion text crosses this boundary. Fire-and-forget.
pub fn record_token_usage(input_tokens: i64, output_tokens: i64) {
    if input_tokens < 0 || output_tokens < 0 {
        return;
    }
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
        if !endpoint_is_allowed(&config.endpoint()) {
            return None;
        }
        let client = reqwest::Client::builder()
            .default_headers(config.headers()?)
            .timeout(REQUEST_TIMEOUT)
            // Do not follow an HTTPS endpoint's redirect to plaintext HTTP.
            .redirect(reqwest::redirect::Policy::none())
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

/// Permit HTTPS destinations and loopback-only HTTP for local diagnostics.
fn endpoint_is_allowed(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() == "https" {
        return url.host_str().is_some();
    }
    if url.scheme() != "http" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if host == "localhost" {
        return true;
    }
    host.trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
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
            config::resolve(cfg!(test) || crate::observability::opted_out()).and_then(
                |mut config| {
                    if let Ok(api_url) = std::env::var(API_URL_ENV) {
                        config.api_url = api_url.trim_end_matches('/').to_owned();
                    }
                    Tracker::new(&config).ok_or(AnalyticsStatus::InvalidConfig)
                },
            )
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
    spawn_tracked_on(&runtime, async move {
        let _ = tracker
            .deliver(&Payload::track(name, Some(&user_id), properties))
            .await;
    });
}

/// Spawn a best-effort analytics delivery that [`flush_pending`] waits for.
///
/// For callers that send an event themselves (sign-out, application start)
/// rather than through a fire-and-forget `record_*` helper: a bare
/// `tokio::spawn` is aborted when its runtime drops at exit, losing the event
/// with no chance to drain it. Does nothing outside a tokio runtime.
pub fn spawn_tracked(delivery: impl std::future::Future<Output = ()> + Send + 'static) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        spawn_tracked_on(&runtime, delivery);
    }
}

fn spawn_tracked_on(
    runtime: &tokio::runtime::Handle,
    delivery: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let pending = PendingEvent::start();
    runtime.spawn(async move {
        // Moved in so it drops with the task: on completion, and equally if
        // the task is aborted or its runtime shuts down first.
        let _pending = pending;
        delivery.await;
    });
}

/// One event counted in [`PENDING_EVENTS`] for as long as this lives.
///
/// The count is released in `Drop` rather than after the delivery's `.await`:
/// a task cancelled mid-delivery (an aborted task, a dropped runtime) never
/// runs the code after its await point, and a leaked count would make every
/// later [`flush_pending`] wait out its whole timeout for an event that is
/// already gone.
struct PendingEvent;

impl PendingEvent {
    fn start() -> Self {
        PENDING_EVENTS.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for PendingEvent {
    fn drop(&mut self) {
        PENDING_EVENTS.fetch_sub(1, Ordering::AcqRel);
        PENDING_EVENTS_CHANGED
            .get_or_init(tokio::sync::Notify::new)
            .notify_waiters();
    }
}
