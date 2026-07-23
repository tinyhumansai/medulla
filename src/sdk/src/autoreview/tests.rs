use std::path::PathBuf;

use super::*;

fn agent(id: &str, availability: &str) -> AgentDescriptor {
    AgentDescriptor {
        id: id.into(),
        availability: availability.into(),
        ..Default::default()
    }
}

#[test]
fn reviewer_selection_excludes_implementer_and_offline_agents() {
    let roster = vec![
        agent("implementer", "idle"),
        agent("z-reviewer", "online"),
        agent("a-offline", "offline"),
    ];
    assert_eq!(
        select_reviewer(&roster, "implementer").unwrap().id,
        "z-reviewer"
    );
    assert_eq!(
        select_reviewer(&[agent("implementer", "online")], "implementer"),
        Err(ReviewError::NoIndependentReviewer("implementer".into()))
    );
}

#[test]
fn instruction_composition_includes_contract_diff_and_verdict_shape() {
    let request = ReviewRequest {
        task_id: "task-7".into(),
        implementer_id: "dev-1".into(),
        reviewer_id: "dev-2".into(),
        workspace: PathBuf::from("/repo"),
        touched_paths: vec![PathBuf::from("src/lib.rs")],
        contract: ReviewContract {
            outcome: "Return the exact result".into(),
            non_goals: vec!["Do not change the wire format".into()],
            verify: vec!["cargo test -p core".into()],
        },
        diff: "diff --git a/src/lib.rs b/src/lib.rs\n+fixed".into(),
    };
    let instruction = compose_instruction(&request);
    assert!(instruction.starts_with("MEDULLA_AUTOREVIEW target=task-7"));
    assert!(instruction.contains("agent `dev-2`"));
    assert!(instruction.contains("implementer `dev-1` MUST NOT"));
    assert!(instruction.contains("Do not change the wire format"));
    assert!(instruction.contains("cargo test -p core"));
    assert!(instruction.contains("diff --git a/src/lib.rs"));
    assert!(instruction.contains("APPROVE"));
    assert!(instruction.contains("FINDINGS:"));
    assert_eq!(review_target(&instruction), Some("task-7"));
}

#[test]
fn contract_parser_supports_structured_and_plain_instructions() {
    let structured = contract_from_instruction(
        "Outcome: ship safely\nNon-goals:\n- no schema changes\nVerify:\n- cargo test\n- make check",
    )
    .unwrap();
    assert_eq!(structured.outcome, "ship safely");
    assert_eq!(structured.non_goals, ["no schema changes"]);
    assert_eq!(structured.verify, ["cargo test", "make check"]);

    let plain = contract_from_instruction("Fix the flaky test").unwrap();
    assert_eq!(plain.outcome, "Fix the flaky test");
    assert!(plain.non_goals.is_empty());
    assert!(contract_from_instruction(" \n ").is_none());
}

#[test]
fn verdict_parser_requires_the_structured_terminal_shape() {
    assert_eq!(parse_verdict("APPROVE"), Some(ReviewVerdict::Approve));
    assert_eq!(
        parse_verdict("FINDINGS:\n- missing test\n- unsafe fallback"),
        Some(ReviewVerdict::Findings(vec![
            "missing test".into(),
            "unsafe fallback".into()
        ]))
    );
    assert_eq!(parse_verdict("looks good"), None);
    assert_eq!(parse_verdict("FINDINGS:"), None);
    assert_eq!(ReviewVerdict::Approve.badge(), "✓ reviewed");
}
