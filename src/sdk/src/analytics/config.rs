//! Resolution of the OpenPanel coordinates a build ships with.
//!
//! Pure functions over their inputs, so the defaults, blank-value handling, and
//! the opt-out precedence are testable without touching the process
//! environment or rebuilding with different `option_env!` values.

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};

use super::types::AnalyticsStatus;

/// OpenPanel's ingestion API for the Medulla project, unless a build overrides
/// it through `MEDULLA_OPENPANEL_API_URL`.
pub(super) const DEFAULT_API_URL: &str = "https://panel.tinyhumans.ai/api";
/// The public OpenPanel client id for the Medulla project, unless a build
/// overrides it through `MEDULLA_OPENPANEL_CLIENT_ID`.
pub(super) const DEFAULT_CLIENT_ID: &str = "781d9ce2-62ec-4059-a093-152c88400576";

/// The resolved OpenPanel coordinates a tracker is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenPanelConfig {
    pub(super) api_url: String,
    pub(super) client_id: String,
}

impl OpenPanelConfig {
    /// Resolve the build-time values, falling back to the Medulla project's
    /// defaults. A blank value counts as absent, because CI exports an empty
    /// variable when the backing Actions variable is unset.
    pub(super) fn from_build(api_url: Option<&str>, client_id: Option<&str>) -> Self {
        Self {
            api_url: non_blank(api_url)
                .unwrap_or_else(|| DEFAULT_API_URL.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            client_id: non_blank(client_id).unwrap_or_else(|| DEFAULT_CLIENT_ID.to_owned()),
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

/// Decide whether this process sends analytics, and with what coordinates.
///
/// The opt-out wins over everything; otherwise analytics is active with the
/// build's (or the Medulla project's default) coordinates.
pub(super) fn resolve(
    opted_out: bool,
    api_url: Option<&str>,
    client_id: Option<&str>,
) -> Result<OpenPanelConfig, AnalyticsStatus> {
    if opted_out {
        return Err(AnalyticsStatus::Disabled);
    }
    Ok(OpenPanelConfig::from_build(api_url, client_id))
}

/// A trimmed, owned copy of `value`, or `None` when it is absent or blank.
fn non_blank(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
