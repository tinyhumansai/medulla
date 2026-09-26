//! What one run says about itself, as labelled rows.
//!
//! [`super::inspect::node_detail`] answers "what does this step declare"; this
//! answers the question beside it — "what was this run, and what was it asked to
//! do". They are deliberately the same shape ([`DetailRow`]) so one renderer
//! draws both.
//!
//! The judgement here is about *ordering and inclusion*, not formatting. A run
//! record carries a dozen fields and an operator scanning one wants them in a
//! fixed order, worst news first: what it is doing now, what it was told to do,
//! and only then the timings. Colour, width, and wrapping stay with the app
//! crate, as everywhere else in [`crate::ui`].

use crate::ui::util::clock;
use crate::workflows::{RunOrigin, RunRecord, RunStatus};

use super::inspect::DetailRow;
use super::rows::status_label;

/// The most characters of one input value a row carries.
///
/// A run's inputs are its identity, so they are shown rather than counted — but
/// a pasted instruction can be thousands of characters and would push every row
/// below it off a pane. Cut here rather than at render time so every surface
/// agrees on where the cut is.
const INPUT_VALUE_CHARS: usize = 240;

/// Everything one run says about itself, in reading order.
///
/// Assembled as rows rather than as a formatted block because the two surfaces
/// that show a run — the graph's step preview and the node inspector — have very
/// different widths, and a pre-formatted block would be wrong in at least one of
/// them.
pub fn run_overview(run: &RunRecord) -> Vec<DetailRow> {
    let mut rows = vec![DetailRow {
        label: "status".into(),
        value: status_line(run),
    }];
    rows.extend(input_rows(run));
    if let Some(origin) = &run.origin {
        rows.push(DetailRow {
            label: "started by".into(),
            value: origin_line(origin),
        });
        if let Some(workspace) = &origin.workspace {
            rows.push(DetailRow {
                label: "in".into(),
                value: workspace.clone(),
            });
        }
    }
    rows.push(DetailRow {
        label: "timing".into(),
        value: timing_line(run),
    });
    rows.push(DetailRow {
        label: "progress".into(),
        value: progress_line(run),
    });
    if !run.pending_approvals.is_empty() {
        rows.push(DetailRow {
            label: "awaiting".into(),
            value: run.pending_approvals.join(", "),
        });
    }
    if let Some(summary) = run
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        rows.push(DetailRow {
            label: "summary".into(),
            value: summary.to_string(),
        });
    }
    if let Some(error) = &run.error {
        rows.push(DetailRow {
            label: "failure".into(),
            value: error.clone(),
        });
    }
    rows.extend(diagnosis_rows(run));
    rows
}

/// The run's state, with what it is parked on when it is parked on something.
fn status_line(run: &RunRecord) -> String {
    let label = status_label(run.status);
    match run.status {
        RunStatus::PendingApproval if !run.pending_approvals.is_empty() => {
            format!("{label} · {}", run.pending_approvals.join(", "))
        }
        _ => label.to_string(),
    }
}

/// One row per declared input the run was given.
///
/// The single most useful thing on this list: two runs of one workflow differ
/// only in what was passed to them. Labelled `input <name>` rather than plain
/// `<name>` so an input called `status` cannot be mistaken for the run's own.
///
/// A run with no declared inputs gets one row saying so, rather than nothing:
/// "this workflow takes no arguments" and "this build does not record them" look
/// identical otherwise, and only one of them is worth investigating.
fn input_rows(run: &RunRecord) -> Vec<DetailRow> {
    let mut rows = Vec::new();
    if run.inputs.is_empty() && run.trigger.is_none() {
        rows.push(DetailRow {
            label: "inputs".into(),
            value: "none".into(),
        });
        return rows;
    }
    for (name, value) in &run.inputs {
        rows.push(DetailRow {
            label: format!("input {name}"),
            value: render_value(value),
        });
    }
    if let Some(trigger) = &run.trigger {
        rows.push(DetailRow {
            label: "trigger".into(),
            value: render_value(trigger),
        });
    }
    rows
}

/// A JSON value as a person reads it, cut to what a detail row has room for.
fn render_value(value: &serde_json::Value) -> String {
    truncate(&value_text(value), INPUT_VALUE_CHARS)
}

/// The readable text of a recorded value, unbounded.
///
/// A value the record bounded on the way in says so in its own shape, carrying
/// the useful text under `preview`; showing the wrapper's machinery would be
/// showing our bookkeeping rather than the caller's argument. Every surface that
/// shows an input goes through here — the detail rows, the history rail, the
/// node preview — so none of them can disagree about it, and each applies its
/// own width afterwards.
///
/// A string is returned as its text — quoting `"main"` into `"\"main\""` is the
/// kind of literalism that makes a pane look like a debugger. Everything else is
/// compact JSON, which is what a number, a flag, or a small list wants.
pub fn value_text(value: &serde_json::Value) -> String {
    // Both the current marker and the one written before the bounding moved to
    // the engine crate — a run record is written once, so older ones stay
    // readable only because this asks the engine rather than matching a literal.
    if let Some(preview) = tinyflows::store::is_truncated(value)
        .then(|| value.get("preview").and_then(serde_json::Value::as_str))
        .flatten()
    {
        // The preview is a prefix of the *serialized* value, so a string's
        // preview opens with the quote JSON wrote — the same literalism this
        // function exists to avoid on an unbounded string.
        return preview.strip_prefix('"').unwrap_or(preview).to_string();
    }
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Cut `text` to `chars` characters, marking that it was cut.
fn truncate(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(chars.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Who started the run, phrased for someone who did not.
fn origin_line(origin: &RunOrigin) -> String {
    let mut parts = Vec::new();
    parts.push(match origin.label.as_deref().map(str::trim) {
        Some(label) if !label.is_empty() => label.to_string(),
        _ => kind_label(&origin.kind).to_string(),
    });
    if let Some(session) = &origin.session {
        parts.push(format!("session {}", short_session(session)));
    }
    parts.join(" · ")
}

/// The operator-facing word for an origin kind.
///
/// An unknown kind passes through verbatim: a record written by a newer build
/// knows about a door this one does not, and showing its own word for it is
/// strictly better than calling it "unknown".
fn kind_label(kind: &str) -> &str {
    match kind {
        RunOrigin::SESSION => "a harness session",
        RunOrigin::CLI => "the command line",
        RunOrigin::OPERATOR => "you",
        other if other.trim().is_empty() => "unknown",
        other => other,
    }
}

/// The tail of a session key, which is the part that tells two apart.
///
/// Session keys are `pty-<uuid>`; the leading segment is the same on every one
/// of them and the pane it lands in is narrow.
pub fn short_session(session: &str) -> String {
    session
        .rsplit('-')
        .next()
        .unwrap_or(session)
        .chars()
        .take(8)
        .collect()
}

/// When the run started, and how long it took or has been going.
fn timing_line(run: &RunRecord) -> String {
    let started = clock(run.started_at as i64);
    match run.duration_ms() {
        Some(elapsed) => format!("started {started} · took {}", human_duration(elapsed)),
        None => format!("started {started} · still running"),
    }
}

/// How far the run got, and how much of it went wrong.
///
/// The failed count is stated even when it is zero for a settled run: "12 steps"
/// on a failed run reads as though the failure were elsewhere, and an operator
/// should not have to open every step to find out that none of them failed (the
/// run timed out, or was cancelled).
fn progress_line(run: &RunRecord) -> String {
    let steps = run.steps.len();
    let mut line = format!("{steps} step{}", if steps == 1 { "" } else { "s" });
    let failed = run.failed_steps();
    if failed > 0 {
        line.push_str(&format!(" · {failed} failed"));
    } else if run.status.is_settled() {
        line.push_str(" · none failed");
    }
    line
}

/// What the run's diagnosis found, as one row per class of problem.
///
/// Counted rather than listed: the per-node detail already appears on the node
/// it belongs to, and this section exists so an operator knows to go looking.
fn diagnosis_rows(run: &RunRecord) -> Vec<DetailRow> {
    let Some(diagnosis) = &run.diagnosis else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    let mut note = |label: &str, count: usize, what: &str| {
        if count > 0 {
            rows.push(DetailRow {
                label: label.into(),
                value: format!("{count} {what}"),
            });
        }
    };
    note(
        "null bindings",
        diagnosis.null_bindings.len(),
        "expression(s) resolved to null",
    );
    note(
        "hidden errors",
        diagnosis.hidden_errors.len(),
        "error(s) a policy swallowed",
    );
    note(
        "never ran",
        diagnosis.never_ran.len(),
        "step(s) the run did not reach",
    );
    note(
        "empty prompts",
        diagnosis.empty_prompts.len(),
        "agent step(s) ran with no prompt",
    );
    rows
}

/// A millisecond span as a person would say it.
pub fn human_duration(millis: u64) -> String {
    let seconds = millis / 1000;
    if seconds < 1 {
        return format!("{millis}ms");
    }
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {}s", seconds % 60);
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
}

#[cfg(test)]
#[path = "run_view_tests.rs"]
mod tests;
