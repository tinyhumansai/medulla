//! Evolution state stays scoped per workspace and per state directory, never
//! shared across stores that should not see each other's claims or proposals.

use std::collections::HashMap;

use serde_json::json;

use super::*;

#[test]
fn discovered_stores_isolate_evolution_state_by_workspace() {
    let root = tempfile::tempdir().unwrap();
    let env = HashMap::from([(
        "MEDULLA_HOME".to_string(),
        root.path().join("home").to_string_lossy().to_string(),
    )]);
    let repo_a = root.path().join("repo-a");
    let repo_b = root.path().join("repo-b");
    let store_a = std::sync::Arc::new(FileWorkflowStore::discover(&env, &repo_a));
    let store_b = std::sync::Arc::new(FileWorkflowStore::discover(&env, &repo_b));
    let record = parse_workflow(&valid_document("deploy"), "deploy").unwrap();
    store_a.save(&record).unwrap();
    store_b.save(&record).unwrap();

    crate::workflows::ops::add_note(
        &(store_a.clone() as std::sync::Arc<dyn WorkflowStore>),
        "deploy",
        "observation",
        "repo A only",
        Vec::new(),
        NoteSource::System,
        Vec::new(),
    )
    .unwrap();

    assert_eq!(store_a.list_notes("deploy").unwrap().len(), 1);
    assert!(store_b.list_notes("deploy").unwrap().is_empty());
}

#[test]
fn independent_instances_over_the_same_state_share_evolution_claims() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let dirs = vec![root.path().join("workflows")];
    let first = FileWorkflowStore::with_state(dirs.clone(), &state);
    let second = FileWorkflowStore::with_state(dirs, &state);
    let first_scope = first.proposal_decision_scope();
    let second_scope = second.proposal_decision_scope();

    assert_eq!(first_scope, second_scope);
    let _guard =
        EvolveGuard::claim(&first_scope, "deploy").expect("the first store instance claims it");
    assert!(
        EvolveGuard::claim(&second_scope, "deploy").is_none(),
        "another instance over the same state must not start a duplicate review"
    );
}

#[test]
fn a_proposal_is_not_published_after_its_base_graph_moves() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut record = parse_workflow(&valid_document("deploy"), "deploy").unwrap();
    store.save(&record).unwrap();
    let base_fingerprint = crate::workflows::fingerprint(&record.graph);
    let proposal = WorkflowProposal {
        id: "proposal".into(),
        workflow_id: "deploy".into(),
        created_at: 1,
        rationale: "change the worker".into(),
        ops: json!([]),
        evidence_runs: Vec::new(),
        note_ids: Vec::new(),
        base_fingerprint: base_fingerprint.clone(),
        verification: None,
        status: ProposalStatus::Pending,
        decided_at: None,
        decision_reason: None,
    };
    record.graph.nodes[1].name = "Changed concurrently".into();
    store.save(&record).unwrap();

    assert!(!store
        .save_proposal_if_fingerprint(&proposal, &base_fingerprint)
        .unwrap());
    assert!(store.get_proposal(&proposal.id).unwrap().is_none());
}
