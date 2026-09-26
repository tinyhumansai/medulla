//! Listing rows for installed workflows and their runs.
//!
//! Neutral rows rather than ratatui widgets, matching the rest of
//! [`crate::ui`]: the SDK decides *what* a surface shows and the app crate
//! decides how it is drawn, so the same data can reach a terminal, a log, or a
//! test without three renderings of the same judgement.

use crate::workflows::{RunRecord, RunStatus, WorkflowSummary};

/// One line about a workflow or one of its runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRow {
    /// Selection key, stable across refreshes so the cursor stays put when a
    /// sibling appears or disappears.
    pub key: String,
    /// The row's primary label.
    pub label: String,
    /// Trailing status text.
    pub detail: String,
    /// Whether this row describes something not currently runnable — a disabled
    /// workflow, a failed run. Rendered dim.
    pub degraded: bool,
}

/// A row per installed workflow.
pub fn workflow_rows(workflows: &[WorkflowSummary]) -> Vec<WorkflowRow> {
    workflows
        .iter()
        .map(|workflow| WorkflowRow {
            key: format!("workflow:{}", workflow.id),
            label: workflow.name.clone(),
            detail: detail_for(workflow),
            degraded: !workflow.enabled,
        })
        .collect()
}

/// The trailing text for a workflow: how it starts, how big it is, and what it
/// does — the three things that decide whether it is the one you want.
fn detail_for(workflow: &WorkflowSummary) -> String {
    let mut parts = Vec::new();
    if !workflow.enabled {
        parts.push("disabled".to_string());
    }
    if let Some(trigger) = &workflow.trigger_kind {
        parts.push(trigger.clone());
    }
    parts.push(format!(
        "{} step{}",
        workflow.node_count,
        if workflow.node_count == 1 { "" } else { "s" }
    ));
    if !workflow.description.is_empty() {
        parts.push(workflow.description.clone());
    }
    parts.join(" · ")
}

/// A row per run, newest first as the store returns them.
pub fn run_rows(runs: &[RunRecord]) -> Vec<WorkflowRow> {
    runs.iter()
        .map(|run| WorkflowRow {
            key: format!("run:{}", run.id),
            label: run.id.clone(),
            detail: run_detail(run),
            // A settled-but-unsuccessful run is history worth dimming; one still
            // going is the thing an operator is watching.
            degraded: matches!(
                run.status,
                RunStatus::Failed | RunStatus::Cancelled | RunStatus::Interrupted
            ),
        })
        .collect()
}

/// Characters of the run's inputs the listing row carries.
///
/// A rail is narrow and the renderer clips, but clipping *here* decides which
/// inputs survive rather than leaving it to whatever column the pane happens to
/// have. Enough for two or three short arguments, which is what identifies a run.
const INPUTS_CHARS: usize = 48;

/// The trailing text for a run: what it was given, where it got to, and what it
/// is waiting on.
///
/// The inputs lead. Every run of one workflow shares its name, its step count,
/// and usually its status too, so a history listing without them is the same
/// sentence repeated — and the argument a run was given is the one thing that
/// tells an operator which run they are looking for.
fn run_detail(run: &RunRecord) -> String {
    let mut parts = Vec::new();
    if let Some(inputs) = input_digest(run) {
        parts.push(inputs);
    }
    parts.push(status_label(run.status).to_string());
    parts.push(format!(
        "{} step{}",
        run.steps.len(),
        if run.steps.len() == 1 { "" } else { "s" }
    ));
    if let Some(elapsed) = run.duration_ms() {
        parts.push(super::run_view::human_duration(elapsed));
    }
    if !run.pending_approvals.is_empty() {
        parts.push(format!("awaiting {}", run.pending_approvals.join(", ")));
    }
    if let Some(error) = &run.error {
        parts.push(error.clone());
    }
    parts.join(" · ")
}

/// The run's inputs as `name=value` pairs, bounded to [`INPUTS_CHARS`].
///
/// `None` when the run recorded none — a workflow that takes no arguments, or a
/// record written before runs kept them.
fn input_digest(run: &RunRecord) -> Option<String> {
    if run.inputs.is_empty() {
        return None;
    }
    let mut digest = String::new();
    for (name, value) in &run.inputs {
        // Through `value_text` so an input the record bounded shows the text it
        // was given rather than the wrapper we stored it in.
        let value = super::run_view::value_text(value);
        let pair = format!("{name}={}", value.replace(['\n', '\r'], " "));
        if !digest.is_empty() {
            digest.push(' ');
        }
        digest.push_str(&pair);
        if digest.chars().count() >= INPUTS_CHARS {
            break;
        }
    }
    if digest.chars().count() > INPUTS_CHARS {
        digest = digest
            .chars()
            .take(INPUTS_CHARS.saturating_sub(1))
            .chain(std::iter::once('…'))
            .collect();
    }
    Some(digest)
}

/// The operator-facing word for a run status.
pub fn status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::PendingApproval => "awaiting approval",
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Interrupted => "interrupted",
    }
}

/// A run id shortened to something a narrow pane has room for.
///
/// Run ids are `run-<uuid>`, which no sidebar or panel title can show whole. The
/// trailing segment is the random part, so the last few characters of it are
/// what distinguish two runs — and they are enough to find the run again with
/// `medulla workflow get-run`, which matches on a prefix of nothing but still
/// gives an operator something to search their history for.
pub fn short_run_id(id: &str) -> String {
    id.rsplit('-')
        .next()
        .unwrap_or(id)
        .chars()
        .take(8)
        .collect()
}

/// A colour name for a run status, in the vocabulary the app crate maps to a
/// theme.
pub fn status_color(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running | RunStatus::PendingApproval => "yellow",
        RunStatus::Succeeded => "green",
        RunStatus::Failed | RunStatus::Cancelled | RunStatus::Interrupted => "red",
    }
}

#[cfg(test)]
#[path = "rows_tests.rs"]
mod tests;
