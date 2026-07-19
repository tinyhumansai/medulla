//! Plain data types for the auth module: the stored [`Credentials`], the OAuth
//! [`Provider`] enum, the loopback-flow [`LoginError`] and [`LoopbackConfig`],
//! and the [`DEFAULT_LOGIN_TIMEOUT`] constant. Behaviour-heavy types (the
//! credential store, the loopback listener) live beside their logic.

use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The default overall wait for the browser round-trip.
pub const DEFAULT_LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// Stored login credentials: the backend they belong to and the bearer JWT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub jwt: String,
}

/// The OAuth identity providers the backend accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Provider {
    #[default]
    Google,
    Github,
    Twitter,
    Discord,
}

impl Provider {
    /// The wire slug used in the login path (`/auth/<slug>/login`).
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::Github => "github",
            Provider::Twitter => "twitter",
            Provider::Discord => "discord",
        }
    }

    /// Parse a provider name (case-insensitive), or `None` if unrecognized.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "google" => Some(Provider::Google),
            "github" => Some(Provider::Github),
            "twitter" => Some(Provider::Twitter),
            "discord" => Some(Provider::Discord),
            _ => None,
        }
    }
}

/// A failure of the loopback login flow.
#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    /// Could not bind / accept on the loopback socket.
    #[error("loopback login I/O error: {0}")]
    Io(#[from] io::Error),
    /// The browser round-trip did not complete within the timeout.
    #[error("timed out waiting for the browser to complete login")]
    Timeout,
    /// The backend redirected back with an `error` parameter.
    #[error("login failed: {0}")]
    Backend(String),
}

/// Knobs for [`run_login_flow`](super::run_login_flow).
#[derive(Debug, Clone)]
pub struct LoopbackConfig {
    /// Overall wait for the browser round-trip.
    pub timeout: Duration,
    /// Skip spawning a browser (still prints the URL to stderr).
    pub no_browser: bool,
}

impl Default for LoopbackConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_LOGIN_TIMEOUT,
            no_browser: false,
        }
    }
}
