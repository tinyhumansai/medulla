//! Rendering a [`WorkSnapshot`] as display
//! rows: the goal, the todo list, the sub-agents, the files touched, and how the
//! run ended.
//!
//! This is the read-only view of what another harness is showing on its own
//! screen. Pure formatting — the `medulla-tui` crate turns the returned
//! [`Line`]s into ratatui spans — and every section is skipped entirely when the
//! harness reported nothing for it, so a sparse snapshot renders as a short
//! panel rather than a page of empty headings.
//!
//! Each list is capped with a `… +k more` tail. The panel shares a pane with a
//! transcript, and an agent with sixty todos must not push everything else off
//! the screen; the rows that survive truncation are the ones still in play.

mod summary;

#[cfg(test)]
mod tests;

pub use summary::{work_chip, work_headline};

use crate::harness_work::{SubagentStatus, WorkItemStatus, WorkSnapshot};
use crate::ui::agents::Line;
use crate::ui::util::{clip, wrap};

/// Rows shown per list before the `… +k more` tail.
const MAX_TODOS: usize = 12;
/// Sub-agent rows shown before the tail. Lower than the todo cap: each one is a
/// whole delegated run, and a fan-out that wide is better read task by task.
const MAX_SUBAGENTS: usize = 8;
/// File rows shown before the tail.
const MAX_FILES: usize = 8;

/// Render the whole work panel for `work`, wrapped to `width` columns.
///
/// Returns an empty vector for an empty snapshot so a caller can decide not to
/// draw the panel at all.
pub fn work_lines(work: &WorkSnapshot, width: usize) -> Vec<Line> {
    if work.is_empty() {
        return Vec::new();
    }
    let cols = width.max(16);
    let mut lines = Vec::new();
    goal_section(work, cols, &mut lines);
    todo_section(work, cols, &mut lines);
    subagent_section(work, cols, &mut lines);
    file_section(work, cols, &mut lines);
    info_section(work, cols, &mut lines);
    outcome_section(work, cols, &mut lines);
    lines
}

/// A dim section heading (`tasks · 2/5 done`).
fn heading(text: String) -> Line {
    Line {
        text,
        color: Some("cyan".into()),
        dim: true,
    }
}

/// The `… +k more` tail shown when a list was capped.
fn more(hidden: usize) -> Line {
    Line {
        text: format!("… +{hidden} more"),
        color: Some("cyan".into()),
        dim: true,
    }
}

/// What the agent says it is trying to do, plus the plan under it.
fn goal_section(work: &WorkSnapshot, cols: usize, lines: &mut Vec<Line>) {
    if let Some(goal) = &work.goal {
        lines.push(heading("goal".into()));
        for row in wrap(goal, cols) {
            lines.push(Line {
                text: row,
                color: Some("magenta".into()),
                dim: false,
            });
        }
    }
    // The plan renders only when it is not simply the todo list under another
    // name — several harnesses write both, and showing the same items twice
    // makes the panel look like there is twice as much work.
    if work.plan.is_empty() || plan_mirrors_todos(work) {
        return;
    }
    lines.push(heading(format!("plan · {} step(s)", work.plan.len())));
    for item in work.plan.iter().take(MAX_TODOS) {
        lines.push(item_line(item.status, item.display(), cols));
    }
    if work.plan.len() > MAX_TODOS {
        lines.push(more(work.plan.len() - MAX_TODOS));
    }
}

/// Whether the plan is the same list of items as the todos.
fn plan_mirrors_todos(work: &WorkSnapshot) -> bool {
    work.plan.len() == work.todos.len()
        && work
            .plan
            .iter()
            .zip(work.todos.iter())
            .all(|(a, b)| a.text == b.text)
}

/// The live todo list, with its progress in the heading.
fn todo_section(work: &WorkSnapshot, cols: usize, lines: &mut Vec<Line>) {
    if work.todos.is_empty() {
        return;
    }
    let (done, total) = work.todo_progress();
    lines.push(heading(format!("tasks · {done}/{total} done")));
    // Cap by dropping settled items first: what is left to do is what the
    // operator is reading the panel for.
    let hidden = work.todos.len().saturating_sub(MAX_TODOS);
    let shown: Vec<_> = if hidden == 0 {
        work.todos.iter().collect()
    } else {
        let mut ordered: Vec<_> = work.todos.iter().collect();
        ordered.sort_by_key(|item| item.status.is_terminal());
        ordered.truncate(MAX_TODOS);
        ordered
    };
    for item in shown {
        lines.push(item_line(item.status, item.display(), cols));
    }
    if hidden > 0 {
        lines.push(more(hidden));
    }
}

/// One checkbox row.
fn item_line(status: WorkItemStatus, text: &str, cols: usize) -> Line {
    Line {
        text: format!("{} {}", status.glyph(), clip(text, cols.saturating_sub(2))),
        color: Some(status.color().into()),
        dim: status.is_terminal(),
    }
}

/// The sub-agents this harness spawned — the layer that is otherwise invisible.
fn subagent_section(work: &WorkSnapshot, cols: usize, lines: &mut Vec<Line>) {
    if work.subagents.is_empty() {
        return;
    }
    let running = work.running_subagents();
    let heading_text = if running > 0 {
        format!("sub-agents · {} · {running} running", work.subagents.len())
    } else {
        format!("sub-agents · {}", work.subagents.len())
    };
    lines.push(heading(heading_text));
    // Running runs first: a finished sub-agent is history, and history is what
    // gets dropped when the list is capped.
    let mut ordered: Vec<_> = work.subagents.iter().collect();
    ordered.sort_by_key(|run| run.status != SubagentStatus::Running);
    let hidden = ordered.len().saturating_sub(MAX_SUBAGENTS);
    for run in ordered.iter().take(MAX_SUBAGENTS) {
        // The type is what distinguishes two runs with similar descriptions, so
        // it rides on the same row rather than a second one.
        let kind = run
            .agent_type
            .as_deref()
            .filter(|kind| !kind.trim().is_empty())
            .map(|kind| format!(" ({kind})"))
            .unwrap_or_default();
        lines.push(Line {
            text: format!(
                "{} {}",
                run.status.glyph(),
                clip(&format!("{}{kind}", run.label()), cols.saturating_sub(2))
            ),
            color: Some(run.status.color().into()),
            dim: run.status != SubagentStatus::Running,
        });
    }
    if hidden > 0 {
        lines.push(more(hidden));
    }
}

/// The files the harness changed, newest first.
fn file_section(work: &WorkSnapshot, cols: usize, lines: &mut Vec<Line>) {
    if work.files.is_empty() {
        return;
    }
    lines.push(heading(format!("files · {} touched", work.files.len())));
    let mut ordered: Vec<_> = work.files.iter().collect();
    ordered.sort_by_key(|file| std::cmp::Reverse(file.at));
    let hidden = ordered.len().saturating_sub(MAX_FILES);
    for file in ordered.iter().take(MAX_FILES) {
        // Repeat edits are counted rather than repeated: five rows for one file
        // says nothing five times.
        let repeats = if file.edits > 1 {
            format!(" ×{}", file.edits)
        } else {
            String::new()
        };
        // Paths are clipped from the left: the tail of a path identifies a file,
        // the head is the same for everything in the repository.
        let room = cols.saturating_sub(2 + repeats.chars().count());
        lines.push(Line {
            text: format!(
                "{} {}{repeats}",
                file.kind.marker(),
                crate::ui::util::clip_left(&file.path, room)
            ),
            color: Some(file.kind.color().into()),
            dim: false,
        });
    }
    if hidden > 0 {
        lines.push(more(hidden));
    }
}

/// What the harness said about its own session.
fn info_section(work: &WorkSnapshot, cols: usize, lines: &mut Vec<Line>) {
    let info = &work.info;
    if info.is_empty() {
        return;
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = &info.model {
        parts.push(model.clone());
    }
    if let Some(mode) = &info.permission_mode {
        parts.push(mode.clone());
    }
    if let Some(branch) = &info.branch {
        parts.push(format!("branch {branch}"));
    }
    if let Some(pull_request) = &info.pull_request {
        parts.push(crate::harness_work::pull_request_label(pull_request));
    }
    if !info.mcp_servers.is_empty() {
        parts.push(format!("mcp {}", info.mcp_servers.join(", ")));
    }
    if !info.tools.is_empty() {
        parts.push(format!("{} tools", info.tools.len()));
    }
    if parts.is_empty() && info.cwd.is_none() {
        return;
    }
    lines.push(heading("session".into()));
    if !parts.is_empty() {
        lines.push(Line {
            text: clip(&parts.join(" · "), cols),
            color: Some("blue".into()),
            dim: false,
        });
    }
    if let Some(cwd) = &info.cwd {
        lines.push(Line {
            text: crate::ui::util::clip_left(cwd, cols),
            color: Some("blue".into()),
            dim: true,
        });
    }
}

/// How the run ended, once it has.
fn outcome_section(work: &WorkSnapshot, cols: usize, lines: &mut Vec<Line>) {
    let Some(outcome) = &work.outcome else {
        return;
    };
    let mut parts = vec![if outcome.ok {
        "ok".to_string()
    } else {
        "failed".to_string()
    }];
    if let Some(turns) = outcome.num_turns {
        parts.push(format!("{turns} turn(s)"));
    }
    if let Some(ms) = outcome.duration_ms {
        parts.push(format!("{:.1}s", ms as f64 / 1000.0));
    }
    if let Some(cost) = outcome.cost_usd {
        parts.push(format!("${cost:.2}"));
    }
    lines.push(heading("outcome".into()));
    lines.push(Line {
        text: clip(&parts.join(" · "), cols),
        color: Some(if outcome.ok { "green" } else { "red" }.into()),
        dim: false,
    });
}
