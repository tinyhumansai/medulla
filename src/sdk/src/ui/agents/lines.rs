//! Transcript rendering: flatten a lane's or task's folded turns into pre-wrapped,
//! styled [`Line`] rows for the detail pane. Owns [`lane_lines`] and [`task_lines`]
//! and the shared block-to-lines walker they both use.

use crate::ui::util::wrap;

use super::rows::ordered_tasks;
use super::types::{AgentLane, AgentRole, Line, TaskState, TurnBlock};

/// Render a run of turn blocks into styled, pre-wrapped display rows.
fn blocks_to_lines(turns: &[TurnBlock], cols: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    for turn in turns {
        let header = format!("{}  {}", crate::ui::util::clock(turn.at), turn.header);
        let header = if header.chars().count() > cols {
            let mut s: String = header.chars().take(cols.saturating_sub(1)).collect();
            s.push('…');
            s
        } else {
            header
        };
        lines.push(Line {
            text: header,
            color: Some(turn.header_color.clone().unwrap_or_else(|| "cyan".into())),
            dim: false,
        });
        if let Some(reasoning) = &turn.reasoning {
            lines.push(Line {
                text: "  · thinking".into(),
                color: Some("yellow".into()),
                dim: true,
            });
            for row in wrap(reasoning, cols.saturating_sub(2)) {
                lines.push(Line {
                    text: format!("  {row}"),
                    color: Some("yellow".into()),
                    dim: true,
                });
            }
        }
        if let Some(content) = &turn.content {
            lines.push(Line {
                text: "  › output".into(),
                color: Some("green".into()),
                dim: true,
            });
            for row in wrap(content, cols) {
                lines.push(Line {
                    text: row,
                    ..Default::default()
                });
            }
        }
        if !turn.tools.is_empty() {
            lines.push(Line {
                text: "  → tools".into(),
                color: Some("blue".into()),
                dim: true,
            });
            for tool in &turn.tools {
                for row in wrap(tool, cols) {
                    lines.push(Line {
                        text: row,
                        color: Some("blue".into()),
                        dim: false,
                    });
                }
            }
        }
        lines.push(Line::default());
    }
    lines
}

/// Flatten a lane's turns into pre-wrapped, styled rows. Agent-identity lanes
/// group turns under each task; others render their flat transcript.
pub fn lane_lines(lane: Option<&AgentLane>, width: usize) -> Vec<Line> {
    let Some(lane) = lane else { return Vec::new() };
    let cols = width.max(20);
    if lane.role == AgentRole::Worker && lane.key.starts_with("agent:") && !lane.tasks.is_empty() {
        let mut lines = Vec::new();
        for task in ordered_tasks(&lane.tasks) {
            lines.push(Line {
                text: format!(
                    "── {} · {} · {} turn(s) ──",
                    task.task_id,
                    task.status.label(),
                    task.turns
                ),
                color: Some(task.status.color().into()),
                dim: false,
            });
            let body = blocks_to_lines(&task.turn_blocks, cols);
            if body.is_empty() {
                lines.push(Line {
                    text: "  (no turns yet)".into(),
                    dim: true,
                    ..Default::default()
                });
            } else {
                lines.extend(body);
            }
        }
        return lines;
    }
    if lane.turns.is_empty() {
        return vec![Line {
            text: "No turns yet.".into(),
            dim: true,
            ..Default::default()
        }];
    }
    blocks_to_lines(&lane.turns, cols)
}

/// The per-task transcript for a task-focused view.
pub fn task_lines(task: &TaskState, width: usize) -> Vec<Line> {
    let cols = width.max(20);
    let mut review_lines = match &task.review {
        Some(crate::autoreview::ReviewVerdict::Approve) => vec![Line {
            text: "✓ reviewed".into(),
            color: Some("green".into()),
            dim: false,
        }],
        Some(crate::autoreview::ReviewVerdict::Findings(findings)) => {
            let mut lines = vec![Line {
                text: format!("✗ findings({})", findings.len()),
                color: Some("red".into()),
                dim: false,
            }];
            lines.extend(findings.iter().map(|finding| Line {
                text: format!("- {finding}"),
                color: Some("red".into()),
                dim: false,
            }));
            lines
        }
        None => Vec::new(),
    };
    if task.turn_blocks.is_empty() {
        review_lines.push(Line {
            text: "No turns yet.".into(),
            dim: true,
            ..Default::default()
        });
        return review_lines;
    }
    review_lines.extend(blocks_to_lines(&task.turn_blocks, cols));
    review_lines
}
