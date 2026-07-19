//! Rendering a drafted profile into `MEDULLA.md` text.
//!
//! The scaffold ships in `docs/templates/MEDULLA.md.tmpl` and is compiled in
//! with `include_str!`, so the shape an operator sees lives in one editable
//! place rather than being spelled out in Rust string literals. Substitution is
//! deliberately dumb (`{{key}}` → value): the template is ours, not user input.

use super::types::DraftedProfile;

/// The scaffold, compiled in from `docs/templates/`.
const TEMPLATE: &str = include_str!("../../../../docs/templates/MEDULLA.md.tmpl");

/// Render a YAML flow-sequence body (`a, b`) from a list, quoting nothing —
/// harness and model ids are bare tokens by convention.
fn flow_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Indent each routing line by two spaces so it sits inside the `routing: |`
/// block scalar. An empty list yields a single placeholder line, keeping the
/// block well-formed rather than emitting a dangling `routing: |`.
fn routing_block(lines: &[String]) -> String {
    let cleaned: Vec<String> = lines
        .iter()
        .flat_map(|line| line.lines().collect::<Vec<_>>())
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    if cleaned.is_empty() {
        return "  TODO: how should work in this workspace be routed?".to_string();
    }
    cleaned
        .iter()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a drafted profile into the `MEDULLA.md` document.
///
/// The result is always parseable by the medulla SDK: empty lists render as
/// empty flow sequences and an empty routing block renders a TODO line, so a
/// stub draft still produces a valid, hand-editable file.
pub fn render_medulla_md(draft: &DraftedProfile) -> String {
    let summary = if draft.summary.trim().is_empty() {
        super::types::STUB_SUMMARY
    } else {
        draft.summary.trim()
    };
    TEMPLATE
        .replace("{{harnesses}}", &flow_list(&draft.harnesses))
        .replace("{{models_reasoning}}", &flow_list(&draft.models_reasoning))
        .replace("{{routing}}", &routing_block(&draft.routing))
        .replace("{{summary}}", summary)
}
