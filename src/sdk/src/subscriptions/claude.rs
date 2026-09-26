//! Claude subscription windows, read from the same OAuth session the local CLI
//! signed in with.
//!
//! Claude publishes the windows an operator actually bumps into — the rolling
//! session window, the seven-day account window, the seven-day window scoped to
//! one model, and the usage-credit spend — from one endpoint, and nowhere on
//! disk. So this probe borrows the CLI's own access token rather than asking
//! for a second credential: the token is already on the machine, already
//! scoped to this account, and a meter that needed its own login would not get
//! switched on.
//!
//! Fail-open throughout. A machine that keeps its Claude credentials in an OS
//! keychain has no token file, an expired token gets a 401, and either way the
//! meters come back with a note rather than an error that would take the whole
//! sample down with them.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::config::MeterId;

use super::time::parse_rfc3339;
use super::types::MeterReading;

/// Where the usage windows are published.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// The beta header the OAuth-scoped endpoints require.
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// How long a probe waits before giving the meter up for this cycle.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Sample every Claude meter, keyed by id.
///
/// Returns one entry per meter the endpoint answered for, plus — when the probe
/// itself could not run — a single explanatory reading the caller spreads
/// across whichever Claude meters are switched on.
pub async fn sample() -> Result<Vec<(MeterId, MeterReading)>, String> {
    let token = access_token().ok_or_else(|| "no local Claude login".to_string())?;
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", OAUTH_BETA)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        // 401 is the common one and it has an action attached, so name it
        // rather than showing the operator a bare status code.
        return Err(match status.as_u16() {
            401 | 403 => "Claude login expired".to_string(),
            code => format!("Claude usage HTTP {code}"),
        });
    }
    let body: Value = response.json().await.map_err(|error| error.to_string())?;
    Ok(readings(&body))
}

/// Map one usage document onto the meters, ignoring anything unrecognized.
///
/// Pure, so the shape of a live response can be pinned in a test without a
/// network. Reads the `limits` array first — it is the form that carries the
/// scoped windows — and falls back to the older top-level window objects for a
/// response that predates it.
pub fn readings(body: &Value) -> Vec<(MeterId, MeterReading)> {
    let mut out: Vec<(MeterId, MeterReading)> = Vec::new();
    if let Some(limits) = body.get("limits").and_then(Value::as_array) {
        for limit in limits {
            let kind = limit
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let id = match kind {
                "session" => MeterId::ClaudeSession,
                "weekly_all" => MeterId::ClaudeWeekly,
                "weekly_scoped" => MeterId::ClaudeScoped,
                _ => continue,
            };
            // A scoped window appears once per model. Keep the first, which is
            // the account's own default surface; a second entry would otherwise
            // overwrite it with whichever model happened to sort last.
            if out.iter().any(|(existing, _)| *existing == id) {
                continue;
            }
            out.push((
                id,
                MeterReading {
                    used_fraction: limit.get("percent").and_then(Value::as_f64).map(percent),
                    resets_at: limit
                        .get("resets_at")
                        .and_then(Value::as_str)
                        .and_then(parse_rfc3339),
                    qualifier: limit
                        .pointer("/scope/model/display_name")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    ..MeterReading::default()
                },
            ));
        }
    }
    for (id, key) in [
        (MeterId::ClaudeSession, "five_hour"),
        (MeterId::ClaudeWeekly, "seven_day"),
    ] {
        if out.iter().any(|(existing, _)| *existing == id) {
            continue;
        }
        let Some(window) = body.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        out.push((
            id,
            MeterReading {
                used_fraction: window
                    .get("utilization")
                    .and_then(Value::as_f64)
                    .map(percent),
                remaining: window.get("remaining_dollars").and_then(Value::as_f64),
                limit: window.get("limit_dollars").and_then(Value::as_f64),
                resets_at: window
                    .get("resets_at")
                    .and_then(Value::as_str)
                    .and_then(parse_rfc3339),
                ..MeterReading::default()
            },
        ));
    }
    if let Some(spend) = body.get("spend").filter(|value| !value.is_null()) {
        let used = money(spend.get("used"));
        let limit = money(spend.get("limit"));
        out.push((
            MeterId::ClaudeCredits,
            MeterReading {
                used_fraction: spend
                    .get("percent")
                    .and_then(Value::as_f64)
                    .map(percent)
                    .or_else(|| match (used, limit) {
                        (Some(used), Some(limit)) if limit > 0.0 => Some(used / limit),
                        _ => None,
                    }),
                remaining: match (used, limit) {
                    (Some(used), Some(limit)) => Some((limit - used).max(0.0)),
                    _ => None,
                },
                limit,
                currency: spend
                    .pointer("/limit/currency")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                // Credits are not a rolling window with a reset the operator can
                // wait out, so no reset time is claimed for them.
                ..MeterReading::default()
            },
        ));
    }
    out
}

/// A minor-unit money object (`{amount_minor, exponent}`) as a decimal amount.
fn money(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let minor = value.get("amount_minor").and_then(Value::as_f64)?;
    let exponent = value.get("exponent").and_then(Value::as_i64).unwrap_or(2);
    Some(minor / 10f64.powi(exponent as i32))
}

/// A 0..=100 percentage as a 0..=1 fraction, clamped.
fn percent(value: f64) -> f64 {
    (value / 100.0).clamp(0.0, 1.0)
}

/// The local CLI's Claude access token, if it keeps one in a file.
fn access_token() -> Option<String> {
    let raw = std::fs::read_to_string(credentials_path()?).ok()?;
    let doc: Value = serde_json::from_str(&raw).ok()?;
    doc.pointer("/claudeAiOauth/accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

/// `$CLAUDE_CONFIG_DIR/.credentials.json`, else `~/.claude/.credentials.json`.
fn credentials_path() -> Option<PathBuf> {
    let home = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(dirs::home_dir()?.join(".claude")))?;
    Some(home.join(".credentials.json"))
}
