//! The OpenRouter prepaid balance.
//!
//! A pay-as-you-go key has no window to roll over — what an operator wants on
//! screen is how much is left before requests start failing. OpenRouter states
//! the lifetime credit purchased and the lifetime usage; the difference is the
//! balance, and the ratio is what a bar can be drawn against.

use std::time::Duration;

use serde_json::Value;

use super::types::MeterReading;

/// Where the balance is published.
const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";

/// The environment variable the key is read from unless config names another.
pub const DEFAULT_KEY_ENV: &str = "OPENROUTER_API_KEY";

/// How long a probe waits before giving the meter up for this cycle.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Sample the balance using the key in `key_env`.
pub async fn sample(key_env: Option<&str>) -> Result<MeterReading, String> {
    let variable = key_env.unwrap_or(DEFAULT_KEY_ENV);
    let key = std::env::var(variable)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{variable} not set"))?;
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(CREDITS_URL)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 | 403 => "OpenRouter key rejected".to_string(),
            code => format!("OpenRouter HTTP {code}"),
        });
    }
    let body: Value = response.json().await.map_err(|error| error.to_string())?;
    reading(&body).ok_or_else(|| "OpenRouter sent no credit totals".to_string())
}

/// Map a credits document onto the balance reading.
pub fn reading(body: &Value) -> Option<MeterReading> {
    let data = body.get("data").unwrap_or(body);
    let purchased = data.get("total_credits").and_then(Value::as_f64)?;
    let used = data
        .get("total_usage")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Some(MeterReading {
        used_fraction: (purchased > 0.0).then(|| (used / purchased).clamp(0.0, 1.0)),
        remaining: Some((purchased - used).max(0.0)),
        limit: (purchased > 0.0).then_some(purchased),
        currency: Some("USD".into()),
        ..MeterReading::default()
    })
}
