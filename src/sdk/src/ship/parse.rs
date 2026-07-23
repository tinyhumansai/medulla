//! Parsers for the stable JSON projections requested from GitHub CLI.

use serde::Deserialize;

use super::{CheckState, PrSummary, ShipError};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    number: u64,
    title: String,
    head_ref_name: String,
    url: String,
    #[serde(default)]
    status_check_rollup: Vec<RawCheck>,
}

#[derive(Deserialize)]
struct RawCheck {
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

#[derive(Deserialize)]
struct ThreadEnvelope {
    data: ThreadData,
}

#[derive(Deserialize)]
struct ThreadData {
    repository: ThreadRepository,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadRepository {
    pull_request: ThreadPullRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadPullRequest {
    review_threads: ThreadConnection,
}

#[derive(Deserialize)]
struct ThreadConnection {
    nodes: Vec<ReviewThread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewThread {
    is_resolved: bool,
}

/// Parse PR list JSON, leaving thread counts at zero until the thread probe.
pub(super) fn parse_prs(json: &str) -> Result<Vec<PrSummary>, ShipError> {
    let rows: Vec<RawPr> = serde_json::from_str(json)?;
    Ok(rows
        .into_iter()
        .map(|row| PrSummary {
            number: row.number,
            title: row.title,
            head: row.head_ref_name,
            url: row.url,
            checks: classify_checks(&row.status_check_rollup),
            unresolved_threads: 0,
        })
        .collect())
}

/// Count review threads whose GraphQL `isResolved` flag is false.
pub(super) fn parse_unresolved_threads(json: &str) -> Result<usize, ShipError> {
    let envelope: ThreadEnvelope = serde_json::from_str(json)?;
    Ok(envelope
        .data
        .repository
        .pull_request
        .review_threads
        .nodes
        .iter()
        .filter(|thread| !thread.is_resolved)
        .count())
}

/// Reduce GitHub's heterogeneous check objects into one actionable state.
fn classify_checks(checks: &[RawCheck]) -> CheckState {
    if checks.is_empty() {
        return CheckState::Pending;
    }
    let mut pending = false;
    for check in checks {
        let value = check
            .conclusion
            .as_deref()
            .or(check.state.as_deref())
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(
            value.as_str(),
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
        ) {
            return CheckState::Failing;
        }
        let status = check.status.as_deref().unwrap_or_default();
        pending |= value.is_empty()
            || matches!(
                status.to_ascii_uppercase().as_str(),
                "QUEUED" | "IN_PROGRESS" | "PENDING" | "WAITING" | "REQUESTED"
            );
    }
    if pending {
        CheckState::Pending
    } else {
        CheckState::Green
    }
}
