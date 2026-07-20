//! Read-only view-model helpers for the agent-harness contract: a compact task
//! board rendering for a [`HarnessStatus`] payload, and a one-line budget note
//! for an agent's [`AgentBudgetMetadata`] seat stamp. Pure formatting only — the
//! `medulla-tui` crate turns the returned [`Line`]s / strings into ratatui spans.
//!
//! Both surfaces are additive and degrade to nothing when their payload is
//! absent (an empty board yields an empty `Vec`; a descriptor with no
//! `metadata.budget` yields `None` upstream). Seat display is strictly read-only:
//! seat CRUD stays a backend REST concern and is not modelled here.

use crate::harness_contract::{AgentBudgetMetadata, HarnessStatus, TrackedTask, TrackedTaskStatus};
use crate::ui::agents::Line;
use crate::ui::util::clip;

/// The status glyph for a tracked task, mirroring the lane-marker vocabulary used
/// elsewhere in the Agents view (filled = active work, ○ = terminal).
pub fn task_glyph(status: TrackedTaskStatus) -> char {
    match status {
        TrackedTaskStatus::Open => '○',
        TrackedTaskStatus::Active => '●',
        TrackedTaskStatus::Blocked => '◑',
        TrackedTaskStatus::Done => '✓',
        TrackedTaskStatus::Cancelled => '⊘',
    }
}

/// The display colour name for a tracked task status (consumed by the TUI's
/// `color()` mapping — same palette the task-lane rows use).
pub fn task_color(status: TrackedTaskStatus) -> &'static str {
    match status {
        TrackedTaskStatus::Open => "gray",
        TrackedTaskStatus::Active => "yellow",
        TrackedTaskStatus::Blocked => "red",
        TrackedTaskStatus::Done => "green",
        TrackedTaskStatus::Cancelled => "gray",
    }
}

/// A compact `open 2 · active 1 · blocked 1 · done 3` summary of a task board,
/// omitting any status with a zero count. Returns an empty string for an empty
/// board so callers can skip the row entirely.
pub fn task_board_summary(tasks: &[TrackedTask]) -> String {
    if tasks.is_empty() {
        return String::new();
    }
    let count = |s: TrackedTaskStatus| tasks.iter().filter(|t| t.status == s).count();
    let parts = [
        ("open", count(TrackedTaskStatus::Open)),
        ("active", count(TrackedTaskStatus::Active)),
        ("blocked", count(TrackedTaskStatus::Blocked)),
        ("done", count(TrackedTaskStatus::Done)),
        ("cancelled", count(TrackedTaskStatus::Cancelled)),
    ];
    parts
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(label, n)| format!("{label} {n}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Render a [`HarnessStatus`] task board into compact, styled display rows: a
/// dim header with the per-status counts, then one `glyph title` row per task
/// (titles ellipsized to `width`). Empty board ⇒ empty `Vec` (renders nothing).
pub fn task_board_lines(status: &HarnessStatus, width: usize) -> Vec<Line> {
    if status.tasks.is_empty() {
        return Vec::new();
    }
    let cols = width.max(12);
    let mut lines = Vec::with_capacity(status.tasks.len() + 1);
    lines.push(Line {
        text: format!("tasks · {}", task_board_summary(&status.tasks)),
        color: Some("cyan".into()),
        dim: true,
    });
    for task in &status.tasks {
        let glyph = task_glyph(task.status);
        // Reserve two columns for the glyph + space.
        let title = clip(&task.title, cols.saturating_sub(2));
        lines.push(Line {
            text: format!("{glyph} {title}"),
            color: Some(task_color(task.status).into()),
            dim: false,
        });
    }
    lines
}

/// Compact token count that scales into millions (`980` · `1.2k` · `34k` · `1.2M`).
/// Mirrors the TS `formatTokens` in `core/budgetRoster.ts` so the TUI note reads
/// the same as the orchestrator-facing budget note.
fn fmt_headroom(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", (tokens as f64 / 1_000.0).round() as u64)
    } else {
        tokens.to_string()
    }
}

/// A one-line, read-only budget note for an agent's seat stamp, e.g.
/// `Claude Max 5× · 1.2M left` or `Claude Max 5× · exhausted`. Uses the
/// human-facing `plan_label` to match the existing orchestrator budget note.
pub fn budget_note(budget: &AgentBudgetMetadata) -> String {
    if budget.exhausted {
        format!("{} · exhausted", budget.plan_label)
    } else {
        format!(
            "{} · {} left",
            budget.plan_label,
            fmt_headroom(budget.headroom_tokens)
        )
    }
}

#[cfg(test)]
mod tests;
