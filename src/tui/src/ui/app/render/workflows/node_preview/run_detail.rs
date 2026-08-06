//! Rendering of persisted run evidence for the selected workflow node.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use medulla::ui::workflows::PlacedEdge;
use medulla::workflows::{RunRecord, RunStatus};

use super::kinds::labelled_value;

/// A compact header saying what this run is, above the node evidence.
///
/// Three things a step's evidence cannot say for itself: how the run as a whole
/// went, what it was started with, and who started it. Without them the pane
/// answered "what did this step do" while leaving "in aid of what" — which is
/// the question an operator opening a run has — to be reconstructed from the
/// rail label.
///
/// Compact on purpose: the full account is the inspector's (`i`), and this pane
/// belongs to the step under the cursor.
pub(super) fn run_header(run: &RunRecord) -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut headline = vec![
        Span::styled("run  ", dim),
        Span::styled(
            medulla::ui::workflows::rows::short_run_id(&run.id),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" · {}", medulla::ui::workflows::status_label(run.status)),
            Style::default().fg(super::super::super::color(
                medulla::ui::workflows::status_color(run.status),
            )),
        ),
    ];
    headline.push(Span::styled(
        match run.duration_ms() {
            Some(elapsed) => format!(
                " · {} · {} step{}",
                medulla::ui::workflows::human_duration(elapsed),
                run.steps.len(),
                if run.steps.len() == 1 { "" } else { "s" }
            ),
            None => format!(
                " · running · {} step{}",
                run.steps.len(),
                if run.steps.len() == 1 { "" } else { "s" }
            ),
        },
        dim,
    ));
    let mut lines = vec![Line::from(headline)];

    // `name=value`, joined — the inputs are what tell two runs of one workflow
    // apart, and on one line they read as the arguments they are.
    if !run.inputs.is_empty() {
        let digest = run
            .inputs
            .iter()
            .map(|(name, value)| format!("{name}={}", input_value(value)))
            .collect::<Vec<_>>()
            .join(" · ");
        lines.push(Line::from(vec![
            Span::styled("in   ", dim),
            Span::styled(digest, Style::default().fg(Color::Cyan)),
        ]));
    }
    if let Some(origin) = &run.origin {
        let mut from = origin.label.clone().unwrap_or_else(|| origin.kind.clone());
        if let Some(session) = &origin.session {
            from.push_str(&format!(
                " · session {}",
                medulla::ui::workflows::short_session(session)
            ));
        }
        lines.push(Line::from(vec![
            Span::styled("from ", dim),
            Span::raw(from),
        ]));
    }
    if let Some(summary) = run
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        lines.push(Line::from(vec![
            Span::styled("said ", dim),
            Span::raw(summary.to_string()),
        ]));
    }
    lines
}

/// One input value, short enough to sit beside five others on a line.
fn input_value(value: &Value) -> String {
    // The SDK's reading of the value, so a bounded input shows its preview here
    // as it does in the detail rows rather than the wrapper's metadata.
    let flat = medulla::ui::workflows::value_text(value).replace(['\n', '\r'], " ");
    if flat.chars().count() <= INPUT_DIGEST_CHARS {
        return flat;
    }
    let kept: String = flat.chars().take(INPUT_DIGEST_CHARS - 1).collect();
    format!("{kept}…")
}

/// Characters of one input value the headline digest carries.
const INPUT_DIGEST_CHARS: usize = 40;

/// Render what the selected run recorded for one node.
pub(super) fn run_lines(run: &RunRecord, node_id: &str, is_agent: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    match run.steps.iter().rev().find(|step| step.node_id == node_id) {
        Some(step) => {
            if is_agent {
                if let Some(worker) = step.output.as_ref().and_then(agent_worker) {
                    lines.push(Line::from(vec![
                        Span::styled("ran as  ", Style::default().add_modifier(Modifier::DIM)),
                        Span::styled(
                            worker,
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
                match &step.input {
                    Some(input) => lines.extend(labelled_value("prompt", input)),
                    None => lines.push(Line::from(Span::styled(
                        "prompt  unavailable in this older run record",
                        Style::default().add_modifier(Modifier::DIM),
                    ))),
                }
            }
            if let Some(output) = &step.output {
                let label = if is_agent { "output" } else { "result" };
                let value = if is_agent {
                    agent_output(output)
                } else {
                    output.clone()
                };
                lines.extend(labelled_value(label, &value));
            } else if step.status.eq_ignore_ascii_case("error") {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{}  no output was produced",
                        if is_agent { "output" } else { "result" }
                    ),
                    Style::default().fg(Color::Red),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{}  unavailable in this older run record",
                        if is_agent { "output" } else { "result" }
                    ),
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
        }
        None => lines.push(Line::from(Span::styled(
            "○ not reached in this run",
            Style::default().fg(Color::DarkGray),
        ))),
    }

    if let Some(error) = &run.error {
        lines.extend(labelled_value("failure", &Value::String(error.clone())));
    }
    if let Some(diagnosis) = &run.diagnosis {
        for binding in diagnosis
            .null_bindings
            .iter()
            .filter(|binding| binding.node_id == node_id)
        {
            lines.push(warning_line(format!(
                "{} resolved to null · {}",
                binding.location, binding.suggestion
            )));
        }
        for hidden in diagnosis
            .hidden_errors
            .iter()
            .filter(|hidden| hidden.node_id == node_id)
        {
            lines.push(warning_line(hidden.message.clone().unwrap_or_else(|| {
                "error was swallowed by this step's policy".to_string()
            })));
        }
        if diagnosis.empty_prompts.iter().any(|id| id == node_id) {
            lines.push(warning_line("agent ran with an empty prompt".to_string()));
        }
        if let Some(skipped) = diagnosis
            .never_ran
            .iter()
            .find(|skipped| skipped.node_id == node_id)
        {
            lines.push(warning_line(match &skipped.routed_by {
                Some(condition) => format!("not reached; routed away by {condition}"),
                None => "not reached by this run".to_string(),
            }));
        }
    }
    if run.status == RunStatus::Failed {
        lines.push(Line::from(Span::styled(
            "[f] Fix this run via agent",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines
}

/// Pull the worker identity from an agent result envelope.
fn agent_worker(output: &Value) -> Option<String> {
    output
        .as_array()?
        .iter()
        .find_map(|item| item.get("json")?.get("worker")?.as_str())
        .map(str::to_string)
}

/// Pull the harness prose out of the engine's item envelope when available.
fn agent_output(output: &Value) -> Value {
    let replies = output
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("json")?.get("text")?.as_str())
        .map(|text| Value::String(text.to_string()))
        .collect::<Vec<_>>();
    match replies.len() {
        0 => output.clone(),
        1 => replies.into_iter().next().unwrap_or(Value::Null),
        _ => Value::Array(replies),
    }
}

/// A diagnosis line that stands apart from ordinary result data.
fn warning_line(message: String) -> Line<'static> {
    Line::from(Span::styled(
        format!("⚠ {message}"),
        Style::default().fg(Color::Yellow),
    ))
}

/// Summarize the selected node's incoming and outgoing graph connections.
pub(super) fn connection_line(id: &str, edges: &[PlacedEdge]) -> Line<'static> {
    let incoming = edges
        .iter()
        .filter(|edge| edge.to == id)
        .map(|edge| edge.from.as_str())
        .collect::<Vec<_>>();
    let outgoing = edges
        .iter()
        .filter(|edge| edge.from == id)
        .map(|edge| match &edge.label {
            Some(port) => format!("{port} → {}", edge.to),
            None => edge.to.clone(),
        })
        .collect::<Vec<_>>();
    let dim = Style::default().add_modifier(Modifier::DIM);
    Line::from(vec![
        Span::styled("flow  ", dim),
        Span::raw(if incoming.is_empty() {
            "entry".to_string()
        } else {
            incoming.join(", ")
        }),
        Span::styled("  →  ", dim),
        Span::raw(if outgoing.is_empty() {
            "output".to_string()
        } else {
            outgoing.join(", ")
        }),
    ])
}
