//! The OpenPanel coordinates Medulla reports to.
//!
//! Pure functions over their inputs, so the constants, the headers, and the
//! opt-out precedence are testable without touching the process environment.

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};

use super::types::AnalyticsStatus;

/// OpenPanel's ingestion API for the Medulla project.
pub(super) const DEFAULT_API_URL: &str = "https://panel.tinyhumans.ai/api";
/// Runtime endpoint override used by the diagnostic and local integration tests.
pub const API_URL_ENV: &str = "MEDULLA_ANALYTICS_API_URL";
/// The public OpenPanel client id for the Medulla project.
pub(super) const DEFAULT_CLIENT_ID: &str = "781d9ce2-62ec-4059-a093-152c88400576";

/// The resolved OpenPanel coordinates a tracker is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenPanelConfig {
    pub(super) api_url: String,
    pub(super) client_id: String,
}

impl OpenPanelConfig {
    /// The Medulla project's coordinates.
    pub(super) fn medulla() -> Self {
        Self::new(DEFAULT_API_URL, DEFAULT_CLIENT_ID)
    }

    /// Coordinates for an arbitrary ingestion base URL (a trailing `/` is
    /// dropped) and client id; tests point this at a local listener.
    pub(super) fn new(api_url: &str, client_id: &str) -> Self {
        Self {
            api_url: api_url.trim_end_matches('/').to_owned(),
            client_id: client_id.to_owned(),
        }
    }

    /// The ingestion endpoint every payload is posted to — the URL the
    /// `openpanel_rust` SDK calls `OPENPANEL_TRACK_URL`.
    pub(super) fn endpoint(&self) -> String {
        format!("{}/track", self.api_url)
    }

    /// The headers every request carries: the content type and client id the
    /// `openpanel_rust` SDK's `with_default_headers` sets, plus the SDK name
    /// and version OpenPanel records against each event.
    ///
    /// No client secret: the Medulla OpenPanel client is configured to ignore
    /// CORS and secret checks, so a native client authenticates by id alone.
    /// `None` only when a value is not a legal header value.
    pub(super) fn headers(&self) -> Option<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "openpanel-client-id",
            HeaderValue::from_str(&self.client_id).ok()?,
        );
        headers.insert("openpanel-sdk-name", HeaderValue::from_static("medulla"));
        headers.insert(
            "openpanel-sdk-version",
            HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
        );
        Some(headers)
    }
}

/// Decide whether this process sends analytics: the opt-out wins; otherwise
/// analytics is active with the Medulla project's coordinates.
pub(super) fn resolve(opted_out: bool) -> Result<OpenPanelConfig, AnalyticsStatus> {
    if opted_out {
        return Err(AnalyticsStatus::Disabled);
    }
    Ok(OpenPanelConfig::medulla())
}
