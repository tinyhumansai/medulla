//! Render the selected fleet node into pre-wrapped, styled detail rows.
//!
//! This is the uncapped surface: the list is a pointer-sized index, so anything
//! elided there — every budget window, every declared provider, the full
//! template body, the workspace profile — belongs here. Selecting a row is an
//! explicit "show me everything about this one thing" request.

use crate::runtime::fleet::CapacitySnapshot;
use crate::runtime::AgentDescriptor;
use crate::ui::agents::Line;
use crate::ui::util::wrap;

use super::fmt;

/// Detail rows for the node `key` names, or an empty vector when it names
/// nothing this snapshot declares (a stale selection after a refresh).
pub fn fleet_detail(
    capacity: &CapacitySnapshot,
    roster: &[AgentDescriptor],
    key: &str,
    width: usize,
) -> Vec<Line> {
    let cols = width.max(20);
    let Some((kind, id)) = key.split_once(':') else {
        return Vec::new();
    };
    match kind {
        "host" => host_detail(capacity, id, cols),
        "harness" => harness_detail(capacity, id, cols),
        "workspace" => workspace_detail(capacity, id, cols),
        "agent" => agent_detail(capacity, roster, id, cols),
        "template" => template_detail(capacity, id, cols),
        _ => Vec::new(),
    }
}

/// A `key: value` row, skipped entirely when the value is blank.
fn field(label: &str, value: impl Into<String>) -> Option<Line> {
    let value = value.into();
    (!value.trim().is_empty()).then(|| Line {
        text: format!("{label}: {value}"),
        color: None,
        dim: false,
    })
}

/// A dim section heading.
fn heading(text: &str) -> Line {
    Line {
        text: text.to_string(),
        color: Some("gray".into()),
        dim: true,
    }
}

/// A wrapped free-text block under its own heading.
fn block(title: &str, body: &str, cols: usize) -> Vec<Line> {
    if body.trim().is_empty() {
        return Vec::new();
    }
    let mut lines = vec![Line::default(), heading(title)];
    lines.extend(wrap(body, cols).into_iter().map(|text| Line {
        text,
        ..Default::default()
    }));
    lines
}

fn host_detail(capacity: &CapacitySnapshot, id: &str, cols: usize) -> Vec<Line> {
    let Some(host) = capacity.host(id) else {
        return Vec::new();
    };
    let mut lines: Vec<Line> = [
        field("host", &host.name),
        field("id", &host.id),
        field("availability", &host.availability),
        field("address", host.address.clone().unwrap_or_default()),
        field("resources", fmt::resources(host.resources.as_ref())),
    ]
    .into_iter()
    .flatten()
    .collect();

    let harnesses: Vec<_> = capacity
        .harnesses
        .iter()
        .filter(|h| h.host_id == host.id)
        .collect();
    if !harnesses.is_empty() {
        lines.push(Line::default());
        lines.push(heading("harnesses"));
        for harness in harnesses {
            lines.push(Line {
                text: format!(
                    "  {} · {}",
                    harness.kind,
                    if harness.ready { "ready" } else { "not ready" }
                ),
                color: Some(if harness.ready { "blue" } else { "red" }.into()),
                dim: false,
            });
        }
    }
    lines.extend(metadata_lines(&host.metadata, cols));
    lines
}

fn harness_detail(capacity: &CapacitySnapshot, id: &str, cols: usize) -> Vec<Line> {
    let Some(harness) = capacity.harness(id) else {
        return Vec::new();
    };
    let host = capacity.host(&harness.host_id);
    let mut lines: Vec<Line> = [
        field("harness", &harness.kind),
        field("id", &harness.id),
        field(
            "host",
            host.map(|h| h.name.clone())
                .unwrap_or_else(|| format!("{} (undeclared)", harness.host_id)),
        ),
        field("availability", &harness.availability),
        field(
            "ready",
            if harness.ready {
                "yes".to_string()
            } else {
                harness
                    .ready_reason
                    .clone()
                    .map(|r| format!("no — {r}"))
                    .unwrap_or_else(|| "no".into())
            },
        ),
        field("providers", harness.providers.join(", ")),
        field("templates", harness.template_ids.join(", ")),
    ]
    .into_iter()
    .flatten()
    .collect();

    if !harness.budgets.is_empty() {
        lines.push(Line::default());
        lines.push(heading("budgets"));
        for budget in &harness.budgets {
            // Colour by headroom, not by presence: a seat with 4% left is the
            // reason a delegation will stall, and it should read that way.
            let color = match budget.fraction_remaining() {
                Some(f) if f < 0.1 => "red",
                Some(f) if f < 0.35 => "yellow",
                _ => "green",
            };
            lines.push(Line {
                text: format!("  {}", fmt::budget(budget)),
                color: Some(color.into()),
                dim: false,
            });
            if let Some(seat) = &budget.seat {
                lines.push(Line {
                    text: format!("    seat {seat} · {}", budget.source),
                    dim: true,
                    ..Default::default()
                });
            }
        }
    }

    let workspaces: Vec<_> = capacity
        .workspaces
        .iter()
        .filter(|w| w.harness_id == harness.id)
        .collect();
    if !workspaces.is_empty() {
        lines.push(Line::default());
        lines.push(heading("workspaces"));
        for workspace in workspaces {
            lines.push(Line {
                text: format!("  {}", workspace.path),
                color: Some("green".into()),
                dim: false,
            });
        }
    }
    lines.extend(metadata_lines(&harness.metadata, cols));
    lines
}

fn workspace_detail(capacity: &CapacitySnapshot, id: &str, cols: usize) -> Vec<Line> {
    let Some(workspace) = capacity.workspace(id) else {
        return Vec::new();
    };
    let harness = capacity.harness(&workspace.harness_id);
    let mut lines: Vec<Line> = [
        field("workspace", &workspace.name),
        field("path", &workspace.path),
        field("project", workspace.project.clone().unwrap_or_default()),
        field(
            "harness",
            harness
                .map(|h| h.kind.clone())
                .unwrap_or_else(|| format!("{} (undeclared)", workspace.harness_id)),
        ),
        field(
            "host",
            harness
                .and_then(|h| capacity.host(&h.host_id))
                .map(|h| h.name.clone())
                .unwrap_or_default(),
        ),
        field("templates", workspace.template_ids.join(", ")),
    ]
    .into_iter()
    .flatten()
    .collect();

    if let Some(profile) = &workspace.profile {
        if !profile.harnesses.is_empty() {
            lines.extend(field("prefers", profile.harnesses.join(", ")));
        }
        lines.extend(block("MEDULLA.md", &profile.instructions, cols));
    }
    lines.extend(metadata_lines(&workspace.metadata, cols));
    lines
}

fn agent_detail(
    capacity: &CapacitySnapshot,
    roster: &[AgentDescriptor],
    id: &str,
    cols: usize,
) -> Vec<Line> {
    let Some(agent) = roster.iter().find(|a| a.id == id) else {
        return Vec::new();
    };
    let placement = capacity.placement(agent);
    let mut lines: Vec<Line> = [
        field("agent", &agent.name),
        field("id", &agent.id),
        field("availability", &agent.availability),
        field("tags", agent.tags.join(", ")),
        field(
            "host",
            placement.host.map(|h| h.name.clone()).unwrap_or_default(),
        ),
        field(
            "harness",
            placement
                .harness
                .map(|h| h.kind.clone())
                .unwrap_or_default(),
        ),
        field(
            "workspace",
            placement
                .workspace
                .map(|w| w.path.clone())
                // A local agent has no workspace by construction, which is a
                // fact worth stating rather than an omission.
                .unwrap_or_else(|| "— (local agent)".into()),
        ),
        field(
            "template",
            placement
                .template
                .map(|t| t.name.clone().unwrap_or_else(|| t.id.clone()))
                .or_else(|| agent.template_id.clone())
                .unwrap_or_default(),
        ),
    ]
    .into_iter()
    .flatten()
    .collect();

    if let Some(harness) = placement.harness {
        if let Some(budget) = fmt::tightest(&harness.budgets) {
            lines.extend(field("budget", fmt::budget(budget)));
        }
    }
    lines.extend(block("description", &agent.description, cols));
    lines.extend(metadata_lines(&agent.metadata, cols));
    lines
}

fn template_detail(capacity: &CapacitySnapshot, id: &str, cols: usize) -> Vec<Line> {
    let Some(template) = capacity.template(id) else {
        return Vec::new();
    };
    let mut lines: Vec<Line> = [
        field(
            "template",
            template.name.clone().unwrap_or_else(|| template.id.clone()),
        ),
        field("id", &template.id),
        field("model", template.model.clone().unwrap_or_default()),
        field("effort", template.effort.clone().unwrap_or_default()),
        field(
            "tools",
            template.tools.clone().unwrap_or_default().join(", "),
        ),
        field("tags", template.tags.join(", ")),
    ]
    .into_iter()
    .flatten()
    .collect();

    lines.extend(block("description", &template.description, cols));
    if let Some(instructions) = &template.instructions {
        lines.extend(block("instructions", instructions, cols));
    }
    if !template.harnesses.is_empty() {
        lines.push(Line::default());
        lines.push(heading("per-harness overrides (also the allowed kinds)"));
        for (kind, over) in &template.harnesses {
            lines.push(Line {
                text: format!(
                    "  {kind}: {}",
                    fmt::join(&[
                        over.model.clone().unwrap_or_default(),
                        over.effort.clone().unwrap_or_default(),
                        over.tools
                            .clone()
                            .map(|t| format!("tools {}", t.join(", ")))
                            .unwrap_or_default(),
                    ])
                ),
                color: Some("blue".into()),
                dim: false,
            });
        }
    }
    lines.extend(metadata_lines(&template.metadata, cols));
    lines
}

/// Opaque deployment metadata, rendered last and dim: medulla never interprets
/// it, but an operator who put it there is the one reading this pane.
fn metadata_lines(metadata: &serde_json::Map<String, serde_json::Value>, cols: usize) -> Vec<Line> {
    if metadata.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![Line::default(), heading("metadata")];
    for (key, value) in metadata {
        let rendered = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        for (i, row) in wrap(&format!("{key}: {rendered}"), cols.saturating_sub(2))
            .into_iter()
            .enumerate()
        {
            lines.push(Line {
                text: format!("  {row}"),
                dim: true,
                color: None,
            });
            let _ = i;
        }
    }
    lines
}
