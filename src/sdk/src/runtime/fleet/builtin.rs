//! The built-in agent-template catalog: a starting set of coding roles every
//! install has, so the Templates surface is useful before anyone declares a
//! catalog of their own.
//!
//! A fleet with no templates is a supported configuration, but an unhelpful
//! first run: provisioning stays free-text and the catalog page has nothing to
//! read. These are the roles that recur in nearly every repository — plan,
//! implement, review, debug, verify, document, resolve conflicts, triage — each
//! declared the same way an operator would declare it, so they are examples as
//! much as defaults.
//!
//! Two rules keep them from getting in the way. They are **additive**: a
//! declared template with the same id wins outright, which is how an operator
//! retunes one without forking the list. And they claim **no harness kind** —
//! an empty `harnesses` map means "runs anywhere", so a built-in template is
//! never the reason a workspace has nothing it may provision.

use serde_json::Map;

use super::{AgentTemplate, CapacitySnapshot};

/// The default template catalog: coding roles available on every install.
///
/// Ordered from the work that comes first (planning) to the work that closes a
/// change out (triage, conflicts), because the catalog is rendered in
/// declaration order and that is the order the roles are used in.
pub fn builtin_templates() -> Vec<AgentTemplate> {
    vec![
        template(
            "plan-writer",
            "Plan Writer",
            "Turns an agreed spec into a step-by-step implementation plan with exact file paths.",
            "Turn the requirements you are given into a comprehensive implementation plan. \
             Write for an engineer with no context for this codebase: exact file paths, the \
             complete change per step, the interface between steps, and the command that \
             verifies each one. Order steps so every one leaves the tree building. Never leave \
             a placeholder or a 'TODO: figure out' — resolve the question by reading the code \
             first. Write the plan out; do not implement it.",
            &["read", "search", "write", "shell"],
            "high",
            &["code", "planning"],
        ),
        template(
            "implementer",
            "Implementer",
            "Implements one scoped task test-first, commits it, and reports what it did.",
            "Implement the one task you are given using strict test-driven development: write \
             the failing test first, make it pass with the smallest change that works, then \
             refactor with the tests green. Follow the conventions of the surrounding code \
             rather than your own defaults. Run the project's build, lint, and test commands \
             before you report. Commit in narrow, scoped commits as you go. Report what you \
             changed, what you verified, and anything you had to assume.",
            &["read", "search", "edit", "write", "shell"],
            "high",
            &["code", "implementation"],
        ),
        template(
            "code-reviewer",
            "Code Reviewer",
            "Reviews a change or branch diff and reports what would break, with file:line evidence.",
            "Review the change you are given against its stated intent and the repository's \
             own standards. Report findings as Critical (correctness, security, data loss), \
             Important (design, missing tests, error handling), or Minor (style, naming), each \
             with file:line evidence and a concrete failure scenario — not a category label. \
             Calibrate: an unproblematic change gets a short review. End with a clear merge \
             verdict. Never modify the working tree.",
            &["read", "search", "shell"],
            "high",
            &["code", "review"],
        ),
        template(
            "debugger",
            "Debugger",
            "Traces a bug or failing test to its root cause before proposing any fix.",
            "Find the root cause of the failure you are given before changing anything. \
             Reproduce it, read the actual error and the code path it names, form one \
             hypothesis at a time and test it, and only then fix. The fix is the smallest \
             change that addresses the cause, landed behind a test that fails without it. \
             Guessing at fixes and re-running is not debugging — if you cannot explain why the \
             bug happens, you are not done investigating.",
            &["read", "search", "edit", "shell"],
            "high",
            &["code", "debugging"],
        ),
        template(
            "verifier",
            "Verifier",
            "Independently proves or disproves a completion claim by running the checks itself.",
            "Verify the claim you are given rather than trusting it. For each assertion, \
             identify the command that would prove it, run that command fresh, and read the \
             full output and exit code. A passing summary someone else reported is not \
             evidence. Report an evidence-backed pass or fail, quoting the output that decided \
             it. Do not fix what you find — report it.",
            &["read", "search", "shell"],
            "medium",
            &["code", "verification"],
        ),
        template(
            "test-writer",
            "Test Writer",
            "Adds the tests a change is missing, covering the branches that actually matter.",
            "Write the tests the code you are given is missing. Cover the behaviour and the \
             branches that would break in production — error paths, boundaries, and the case \
             the change was written for — not lines for a coverage number. Match the \
             repository's test framework, layout, and naming. Every test must fail if the \
             behaviour it names regresses; assert on real output, never on a mock you just \
             configured.",
            &["read", "search", "edit", "write", "shell"],
            "medium",
            &["code", "testing"],
        ),
        template(
            "doc-writer",
            "Doc Writer",
            "Documents modules, types, and public APIs in the repository's own style.",
            "Document the code you are given accurately and in this repository's existing \
             conventions. Cover module and folder purpose, public types, and public functions, \
             including preconditions, side effects, and error conditions. Explain intent and \
             non-obvious behaviour rather than restating the code. Change comments and \
             documentation only — never behaviour.",
            &["read", "search", "edit", "write"],
            "medium",
            &["code", "docs"],
        ),
        template(
            "refactorer",
            "Refactorer",
            "Improves the shape of existing code without changing what it does.",
            "Improve the structure of the code you are given while keeping its behaviour \
             identical. Establish the tests that pin the current behaviour first, then work in \
             small steps that keep them green. Remove duplication, narrow interfaces, and \
             delete dead code; do not add features or change public contracts unless the task \
             says to. If a change would alter behaviour, stop and report it instead.",
            &["read", "search", "edit", "shell"],
            "high",
            &["code", "refactor"],
        ),
        template(
            "merge-resolver",
            "Merge Resolver",
            "Resolves merge, rebase, and cherry-pick conflicts by integrating both intents.",
            "Resolve the git conflicts in the tree you are given. For each one, work out what \
             each side changed and why — read the commits, not just the hunk — and integrate \
             both intents. Never take one side wholesale to make the marker disappear. When \
             the tree is clean, build and run the tests before completing the merge or rebase, \
             and report any conflict whose correct resolution you are unsure of.",
            &["read", "search", "edit", "shell"],
            "high",
            &["code", "git"],
        ),
        template(
            "triager",
            "Triager",
            "Triages an issue or pull request: valid, duplicate, actionable, and what it needs next.",
            "Triage the issue or pull request you are given. Establish whether it is valid, \
             already reported, in scope for this repository, and actionable as written. Ground \
             every conclusion in the code — cite the files that confirm or refute the report. \
             Finish with one recommendation: escalate with an implementation sketch, ask the \
             specific question that unblocks it, or close it with the reason. Do not implement \
             the change.",
            &["read", "search", "shell"],
            "medium",
            &["code", "triage"],
        ),
    ]
}

/// `capacity` with every built-in template it does not already declare.
///
/// Matched on id, and the declared record wins: an operator who ships their own
/// `code-reviewer` gets theirs, not this one. Built-ins are appended after the
/// declared catalog so an operator's own roles read first.
pub fn with_builtin_templates(capacity: &CapacitySnapshot) -> CapacitySnapshot {
    let mut out = capacity.clone();
    for template in builtin_templates() {
        if out.templates.iter().any(|t| t.id == template.id) {
            continue;
        }
        out.templates.push(template);
    }
    out
}

/// One built-in template, with the fields these defaults never set left empty.
///
/// `harnesses` stays empty on purpose: a non-empty map is an allowlist, and a
/// default role that only ran on one CLI kind would silently vanish for
/// everyone on another.
fn template(
    id: &str,
    name: &str,
    description: &str,
    instructions: &str,
    tools: &[&str],
    effort: &str,
    tags: &[&str],
) -> AgentTemplate {
    AgentTemplate {
        id: id.into(),
        name: Some(name.into()),
        description: description.into(),
        instructions: Some(instructions.into()),
        tools: Some(tools.iter().map(|t| (*t).to_string()).collect()),
        model: Some("reasoning".into()),
        effort: Some(effort.into()),
        params: Map::new(),
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        metadata: Map::new(),
        harnesses: Default::default(),
    }
}
