//! Built-in agent templates for common software-development workflows.
//!
//! These declarations give a new installation a useful catalog without
//! provisioning agents or granting them access to a particular harness. An
//! operator may replace the catalog, including with an explicit empty list.

use super::AgentTemplate;

/// Return the small coding-oriented catalog used when no templates are declared.
pub(crate) fn coding_agent_templates() -> Vec<AgentTemplate> {
    vec![
        template(
            "coding-agent",
            "Coding agent",
            "Implements one well-scoped change and validates it in the repository.",
            "Work from the repository's local instructions. Inspect the relevant code, implement the smallest complete change, add focused tests, and run the checks appropriate to the touched surface. Preserve unrelated work and report concrete validation evidence.",
            &["coding", "implementation"],
        ),
        template(
            "code-reviewer",
            "Code reviewer",
            "Reviews a completed change for correctness, regressions, and maintainability.",
            "Review the diff against its requirements and repository conventions. Verify important claims from the code and tests. Report only actionable findings, ordered by severity, with file and line evidence. Do not modify the working tree.",
            &["coding", "review"],
        ),
        template(
            "pr-manager",
            "PR manager",
            "Drives a pull request from its current state to a clean review handoff.",
            "Inspect the pull request, checks, and unresolved feedback. Diagnose failures at their root, make only authorized fixes, validate them locally, and keep commits scoped. Summarize remaining risks and never merge without explicit authority.",
            &["coding", "pull-request", "operations"],
        ),
        template(
            "repo-orchestrator",
            "Repository orchestrator",
            "Triages repository work and delegates bounded tasks to suitable agents.",
            "Build an evidence-based view of open repository work, classify each item, and route concrete bounded tasks to the appropriate specialist. Avoid duplicate work, preserve ownership boundaries, and require validation before recommending merge or completion.",
            &["coding", "repository", "orchestration"],
        ),
    ]
}

/// Build a provider-neutral template with no implicit tool or model restriction.
fn template(
    id: &str,
    name: &str,
    description: &str,
    instructions: &str,
    tags: &[&str],
) -> AgentTemplate {
    AgentTemplate {
        id: id.into(),
        name: Some(name.into()),
        description: description.into(),
        instructions: Some(instructions.into()),
        tools: None,
        model: None,
        effort: None,
        params: serde_json::Map::new(),
        tags: tags.iter().map(|tag| (*tag).into()).collect(),
        metadata: serde_json::Map::new(),
        harnesses: std::collections::BTreeMap::new(),
    }
}
