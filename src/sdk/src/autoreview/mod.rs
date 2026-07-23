//! Fresh-context review policy and instruction composition.
//!
//! This module is deliberately runtime-agnostic: it selects a reviewer other
//! than the implementer, parses the lightweight contract convention already
//! carried by task instructions, and emits an ordinary delegation instruction.

mod types;

#[cfg(test)]
mod tests;

pub use types::{ReviewContract, ReviewError, ReviewRequest, ReviewVerdict};

use crate::runtime::AgentDescriptor;

const MARKER: &str = "MEDULLA_AUTOREVIEW target=";

/// Select a deterministic online reviewer, explicitly excluding the implementer.
pub fn select_reviewer<'a>(
    roster: &'a [AgentDescriptor],
    implementer_id: &str,
) -> Result<&'a AgentDescriptor, ReviewError> {
    roster
        .iter()
        .filter(|agent| agent.id != implementer_id)
        .filter(|agent| {
            !matches!(
                agent.availability.to_ascii_lowercase().as_str(),
                "offline" | "unavailable" | "disabled"
            )
        })
        .min_by(|left, right| left.id.cmp(&right.id))
        .ok_or_else(|| ReviewError::NoIndependentReviewer(implementer_id.to_string()))
}

/// Parse `Outcome:`, `Non-goals:`, and `Verify:` blocks from a task instruction.
///
/// Unstructured instructions remain valid: their first non-empty line becomes
/// the outcome, while omitted sections are represented honestly in the review.
pub fn contract_from_instruction(instruction: &str) -> Option<ReviewContract> {
    let lines: Vec<&str> = instruction.lines().map(str::trim).collect();
    let fallback = lines.iter().find(|line| !line.is_empty())?.to_string();
    let outcome = field_value(&lines, "outcome:").unwrap_or(fallback);
    Some(ReviewContract {
        outcome,
        non_goals: section_items(&lines, "non-goals:"),
        verify: section_items(&lines, "verify:"),
    })
}

/// Compose the ordinary delegated task used for independent review.
pub fn compose_instruction(request: &ReviewRequest) -> String {
    format!(
        "{MARKER}{task}\n\
Delegate this review to agent `{reviewer}`. The implementer `{implementer}` MUST NOT perform or \
answer this review. Start a fresh worker session and inspect only the evidence below.\n\n\
## Contract\nOutcome: {outcome}\nNon-goals:\n{non_goals}\nVerify:\n{verify}\n\n\
## Scope\nWorkspace: {workspace}\nTouched paths:\n{paths}\n\n\
## Exact diff\n```diff\n{diff}\n```\n\n\
## Required verdict\nFinish with exactly one task note in one of these shapes:\n\
APPROVE\n\
FINDINGS:\n- <actionable finding>\n\
Do not approve when verification evidence is missing.",
        task = request.task_id,
        reviewer = request.reviewer_id,
        implementer = request.implementer_id,
        outcome = request.contract.outcome,
        non_goals = list_or_missing(&request.contract.non_goals),
        verify = list_or_missing(&request.contract.verify),
        workspace = request.workspace.display(),
        paths = request
            .touched_paths
            .iter()
            .map(|path| format!("- {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n"),
        diff = request.diff.trim_end(),
    )
}

/// Recover the original task id from a generated review instruction.
pub fn review_target(instruction: &str) -> Option<&str> {
    instruction
        .lines()
        .next()?
        .strip_prefix(MARKER)
        .map(str::trim)
        .filter(|target| !target.is_empty())
}

/// Parse the required verdict shape from a review task note.
pub fn parse_verdict(note: &str) -> Option<ReviewVerdict> {
    let trimmed = note.trim();
    if trimmed == "APPROVE" {
        return Some(ReviewVerdict::Approve);
    }
    let (_, body) = trimmed.split_once("FINDINGS:")?;
    let findings: Vec<String> = body
        .lines()
        .flat_map(|line| line.split(';'))
        .map(|line| line.trim().trim_start_matches(['-', '*']).trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    (!findings.is_empty()).then_some(ReviewVerdict::Findings(findings))
}

fn field_value(lines: &[&str], field: &str) -> Option<String> {
    lines.iter().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.eq_ignore_ascii_case(field.trim_end_matches(':')) && !value.trim().is_empty())
            .then(|| value.trim().to_string())
    })
}

fn section_items(lines: &[&str], heading: &str) -> Vec<String> {
    let Some(start) = lines
        .iter()
        .position(|line| line.eq_ignore_ascii_case(heading))
    else {
        return Vec::new();
    };
    lines[start + 1..]
        .iter()
        .take_while(|line| !line.ends_with(':'))
        .map(|line| line.trim_start_matches(['-', '*']).trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn list_or_missing(items: &[String]) -> String {
    if items.is_empty() {
        "- (none recorded)".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
