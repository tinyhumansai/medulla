//! Crash and error reporting to Sentry.
//!
//! The binary calls [`init`] once, before any runtime starts, and holds the
//! returned [`CrashReportingGuard`] for the life of the process. From then on a
//! panic on any thread is reported, after passing through [`scrub`]'s privacy
//! filter: reports drop the hostname, request data, breadcrumbs, free-form
//! extras, local variables, and source context outright, mask the operator's
//! home directory and any other user's directory in every path, redact
//! bearer/JWT-shaped credentials, and carry no account data beyond the opaque
//! account id (see [`set_user`]). The message and exception text a panic
//! supplies are otherwise kept (capped in length) for diagnostic value, so a
//! panic that itself embeds a prompt or argument can still surface it —
//! privacy-filtered, not a guarantee those values never appear.
//!
//! # Configuration
//!
//! - DSN: the Medulla project's DSN is compiled in as a constant, so every
//!   build reports; `MEDULLA_SENTRY_DSN` at **run** time overrides it.
//! - `MEDULLA_SENTRY_ENVIRONMENT` (runtime) overrides the environment, which
//!   otherwise is `production` for release builds and `development` for debug.
//! - Opt-out: [`DISABLED_ENV`] (`MEDULLA_ANALYTICS_DISABLED`) is the single
//!   switch for everything Medulla reports home, crash reports and product
//!   analytics ([`crate::analytics`]) included; see [`opted_out`]. When it is
//!   set the module is fully inert — no client, no thread, no network.
//!
//! This crate's own unit tests always resolve as opted out, so a test run can
//! never send a real report; the transport is exercised against a local
//! listener instead.

mod config;
mod scrub;
mod transport;
mod types;

#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub use types::{CrashReportingGuard, CrashReportingStatus, TestEventReport};

/// Runtime override for the compiled-in Sentry DSN.
pub const DSN_ENV: &str = "MEDULLA_SENTRY_DSN";
/// Runtime override for the Sentry environment name.
pub const ENVIRONMENT_ENV: &str = "MEDULLA_SENTRY_ENVIRONMENT";
/// Opts this process out of everything Medulla reports home, crash reports
/// included, when set to a truthy value (`1`, `true`, or `yes`,
/// case-insensitively).
pub const DISABLED_ENV: &str = "MEDULLA_ANALYTICS_DISABLED";

/// The crate modules whose frames Sentry marks as the application's own.
const IN_APP_CRATES: &[&str] = &["medulla", "medulla_tui", "medulla_link"];

/// Start crash reporting for this process.
///
/// Call once, from `main`, after the TLS provider is installed and before any
/// async runtime starts, and keep the guard alive until exit. Never fails:
/// with the opt-out set, or an unparseable DSN override, it returns an inert
/// guard and records why in [`status`].
pub fn init() -> CrashReportingGuard {
    let (status, dsn) = resolve_status();
    let _ = STATUS.set(status);
    let Some(dsn) = dsn else {
        return CrashReportingGuard { client: None };
    };

    let home = dirs::home_dir().map(|path| path.to_string_lossy().into_owned());
    let client = sentry::init(sentry::ClientOptions {
        dsn: Some(dsn),
        release: Some(Cow::Owned(config::release())),
        environment: Some(Cow::Owned(config::resolve_environment(
            std::env::var(ENVIRONMENT_ENV).ok().as_deref(),
            cfg!(debug_assertions),
        ))),
        send_default_pii: false,
        attach_stacktrace: false,
        in_app_include: IN_APP_CRATES.to_vec(),
        before_send: Some(Arc::new(move |event| {
            let mut event = scrub::scrub_event(event, home.as_deref());
            // Attach the account id here rather than on a scope: scopes are
            // per-thread hubs, and a panic on a worker that cloned its hub
            // before sign-in would otherwise report no account at all.
            event.user = current_user().map(|id| sentry::User {
                id: Some(id),
                ..Default::default()
            });
            Some(event)
        })),
        transport: Some(Arc::new(transport::factory)),
        ..Default::default()
    });
    if let Some(client) = sentry::Hub::main().client() {
        let _ = CLIENT.set(client);
    }
    CrashReportingGuard {
        client: Some(client),
    }
}

/// Why crash reporting is or is not active in this process.
pub fn status() -> CrashReportingStatus {
    STATUS
        .get()
        .copied()
        .unwrap_or(CrashReportingStatus::Uninitialized)
}

/// Associate later reports with an authenticated account id, or clear it.
///
/// Only the opaque id is ever sent — no email, name, or token. Safe to call
/// whether or not crash reporting is active.
pub fn set_user(user_id: Option<&str>) {
    *user_slot().lock().unwrap_or_else(|e| e.into_inner()) = user_id.map(str::to_owned);
}

/// Send one diagnostic event and wait up to `timeout` for it to be delivered.
///
/// Backs the hidden `medulla sentry-test` command, which verifies ingestion end
/// to end. Errors with the inactive [`CrashReportingStatus`] when there is no
/// client to send through.
pub fn send_test_event(timeout: Duration) -> Result<TestEventReport, CrashReportingStatus> {
    let status = status();
    let Some(client) = CLIENT.get().filter(|client| client.is_enabled()) else {
        return Err(match status {
            CrashReportingStatus::Active => CrashReportingStatus::Uninitialized,
            other => other,
        });
    };
    let hub = sentry::Hub::new(Some(Arc::clone(client)), Arc::new(sentry::Scope::default()));
    let event_id = hub.capture_event(sentry::protocol::Event {
        message: Some("medulla sentry-test: verifying crash-report ingestion".into()),
        level: sentry::Level::Info,
        tags: [("diagnostic".to_owned(), "sentry-test".to_owned())]
            .into_iter()
            .collect(),
        ..Default::default()
    });
    let flushed = client.flush(Some(timeout));
    Ok(TestEventReport {
        event_id,
        flushed,
        http_status: transport::last_status(),
    })
}

/// Whether the operator opted this process out of everything Medulla reports
/// home, through [`DISABLED_ENV`].
///
/// The one opt-out check, shared with product analytics
/// ([`crate::analytics`]) so the two can never disagree about it.
pub fn opted_out() -> bool {
    config::is_opted_out(std::env::var(DISABLED_ENV).ok().as_deref())
}

/// The status [`init`] resolved, set once per process.
static STATUS: OnceLock<CrashReportingStatus> = OnceLock::new();

/// The client initialized by this module, used by diagnostics without relying
/// on later mutations to Sentry's process-global main hub.
static CLIENT: OnceLock<Arc<sentry::Client>> = OnceLock::new();

/// Work out whether to start, and with which DSN, from the process
/// environment. Under this crate's unit tests it always resolves as opted
/// out, so no test can reach the live Sentry project.
fn resolve_status() -> (CrashReportingStatus, Option<sentry::types::Dsn>) {
    if cfg!(test) || opted_out() {
        return (CrashReportingStatus::Disabled, None);
    }
    let raw = config::resolve_dsn(std::env::var(DSN_ENV).ok().as_deref());
    match raw.parse::<sentry::types::Dsn>() {
        Err(_) => (CrashReportingStatus::InvalidDsn, None),
        Ok(dsn) => (CrashReportingStatus::Active, Some(dsn)),
    }
}

/// The process-wide account id attached to every report.
fn user_slot() -> &'static Mutex<Option<String>> {
    static USER: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    USER.get_or_init(|| Mutex::new(None))
}

/// Read the account id to attach, if a signed-in run recorded one.
///
/// Also the identity product analytics attributes its events to, so crash
/// reports and analytics share one account slot set through [`set_user`].
pub(crate) fn current_user() -> Option<String> {
    user_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}
