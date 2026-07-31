//! Unit tests for workflow directory layering, document parsing, and the
//! file-backed store's read/write/delete, run-history, and undo behaviour.
//!
//! The mechanics of snapshot files — ordering, the cap, scoping — are tested
//! next to them in `file/revisions_tests.rs`. What is tested here is the part
//! that matters to a caller: that saving captures history at all, and that
//! rolling back lands where the operator expects.

use std::collections::HashMap;
use std::path::Path;

use serde_json::json;

use super::file::{new_run_record, parse_workflow, validate_graph};
use super::{require, require_run, rollback, undo_last, FileWorkflowStore, WorkflowStore};
use crate::workflows::evolve::EvolveGuard;
use crate::workflows::types::{
    ProposalStatus, RunStatus, WorkflowError, WorkflowProposal, WorkflowRecord,
};
use crate::workflows::NoteSource;

/// A store rooted in a temporary directory, with definitions and runs kept
/// apart the way the discovered layout keeps them.
fn store_in(root: &Path) -> FileWorkflowStore {
    FileWorkflowStore::new(vec![root.join("workflows")], root.join("runs"))
}

/// The smallest document that validates: one trigger, one transform, one edge.
fn valid_document(id: &str) -> String {
    json!({
        "id": id,
        "name": "Greet",
        "description": "says hello",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "greet", "kind": "transform", "name": "greet",
              "config": { "set": { "greeting": "=.item.name" } } }
        ],
        "edges": [
            { "from_node": "t", "from_port": "main", "to_node": "greet", "to_port": "main" }
        ]
    })
    .to_string()
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

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

#[test]
fn parsing_names_a_workflow_by_its_filename_when_the_document_omits_an_id() {
    let document = json!({
        "nodes": [{ "id": "t", "kind": "trigger", "name": "start" }],
        "edges": []
    })
    .to_string();

    let record = parse_workflow(&document, "nightly-sweep").expect("parses");

    assert_eq!(record.id, "nightly-sweep");
    // Name falls back to the id so a listing is never blank.
    assert_eq!(record.name, "nightly-sweep");
    assert!(record.enabled, "workflows are enabled unless opted out");
}

#[test]
fn parsing_migrates_a_document_saved_without_a_schema_version() {
    // A document predating the field must keep loading; the engine's migration
    // runs before deserialization, not after.
    let document = json!({
        "id": "old",
        "nodes": [{ "id": "t", "kind": "trigger", "name": "start" }],
        "edges": []
    })
    .to_string();

    let record = parse_workflow(&document, "old").expect("parses");

    assert_eq!(
        record.graph.schema_version,
        tinyflows::model::CURRENT_SCHEMA_VERSION
    );
}

#[test]
fn parsing_rejects_a_document_that_is_not_an_object() {
    let err = parse_workflow("[]", "list").expect_err("an array is not a workflow");
    assert!(err.contains("object"), "unhelpful message: {err}");
}

#[test]
fn validation_reports_every_failure_not_only_the_first() {
    // A graph with no trigger *and* an edge to a node that does not exist. An
    // author — often an agent editing over a tool call — should learn both in
    // one round-trip.
    let graph = serde_json::from_value(json!({
        "nodes": [{ "id": "a", "kind": "transform", "name": "a" }],
        "edges": [{ "from_node": "a", "to_node": "ghost" }]
    }))
    .unwrap();

    let err = validate_graph("broken", &graph).expect_err("invalid");
    let WorkflowError::Invalid { messages, .. } = err else {
        panic!("expected Invalid, got {err:?}");
    };

    assert!(
        messages.len() >= 2,
        "expected every failure, got {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("missing_trigger")),
        "missing trigger not reported: {messages:?}"
    );
}

#[test]
fn a_home_workflow_overrides_a_project_default_of_the_same_id_in_place() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let project = root.path().join("project");

    write(&project.join("first.json"), &valid_document("first"));
    write(&project.join("shared.json"), &valid_document("shared"));
    let overridden = valid_document("shared").replace("\"Greet\"", "\"Personal greet\"");
    write(&home.join("shared.json"), &overridden);

    let store = FileWorkflowStore::new(vec![project, home], root.path().join("runs"));
    let report = store.load();

    assert!(report.errors.is_empty(), "unexpected: {:?}", report.errors);
    let ids: Vec<&str> = report.workflows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["first", "shared"],
        "an override should keep the position of what it overrides"
    );
    assert_eq!(report.workflows[1].name, "Personal greet");
}

#[test]
fn one_malformed_document_costs_only_itself() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("workflows");
    write(&dir.join("good.json"), &valid_document("good"));
    write(&dir.join("broken.json"), "{ not json");

    let store = store_in(root.path());
    let report = store.load();

    assert_eq!(
        report.workflows.len(),
        1,
        "the good document should survive"
    );
    assert_eq!(report.errors.len(), 1);
    assert!(
        report.errors[0].contains("broken.json"),
        "the error should name the file: {:?}",
        report.errors
    );
}

#[test]
fn a_missing_directory_is_not_an_error() {
    let root = tempfile::tempdir().unwrap();
    let report = store_in(root.path()).load();

    assert!(report.workflows.is_empty());
    assert!(report.errors.is_empty(), "unexpected: {:?}", report.errors);
    assert!(report.dirs.is_empty(), "nothing was read");
}

#[test]
fn saving_then_loading_round_trips_the_host_fields_and_the_graph() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut record = parse_workflow(&valid_document("round"), "round").unwrap();
    record.description = "edited".into();
    record.enabled = false;

    store.save(&record).expect("saves");
    let loaded = require(&store, "round").expect("found");

    assert_eq!(loaded.description, "edited");
    assert!(!loaded.enabled, "enabled must survive the round trip");
    assert_eq!(loaded.graph, record.graph);
    assert_eq!(loaded.trigger_kind().as_deref(), Some("manual"));
}

#[test]
fn saving_refuses_a_graph_the_engine_would_not_compile() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let record = WorkflowRecord {
        id: "no-trigger".into(),
        name: "no trigger".into(),
        description: String::new(),
        enabled: true,
        graph: serde_json::from_value(json!({ "nodes": [], "edges": [] })).unwrap(),
        source_path: None,
    };

    let err = store.save(&record).expect_err("must not persist");

    assert!(matches!(err, WorkflowError::Invalid { .. }), "got {err:?}");
    assert!(
        store.list().unwrap().is_empty(),
        "an invalid graph must not reach the catalog"
    );
}

#[test]
fn deleting_removes_the_file_the_workflow_was_actually_read_from() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let record = parse_workflow(&valid_document("gone"), "gone").unwrap();
    store.save(&record).unwrap();

    store.delete("gone").expect("deletes");

    assert!(store.get("gone").unwrap().is_none());
    let err = store
        .delete("gone")
        .expect_err("deleting twice is an error");
    assert!(matches!(err, WorkflowError::NotFound(_)), "got {err:?}");
}

#[test]
fn deleting_a_repository_default_never_modifies_the_checkout() {
    let root = tempfile::tempdir().unwrap();
    let repository_dir = root.path().join("repo/.medulla/workflows");
    let home_dir = root.path().join("home/workflows");
    let repository_file = repository_dir.join("shared.json");
    write(&repository_file, &valid_document("shared"));
    let store = FileWorkflowStore::new(
        vec![repository_dir, home_dir],
        root.path().join("state/runs"),
    );

    let err = store
        .delete("shared")
        .expect_err("repository defaults are read-only");

    assert!(
        matches!(err, WorkflowError::ReadOnlyDefinition { .. }),
        "got {err:?}"
    );
    assert!(
        repository_file.exists(),
        "the checkout must remain untouched"
    );
    assert!(store.get("shared").unwrap().is_some());
}

#[test]
fn deleting_a_home_definition_uses_its_actual_filename() {
    let root = tempfile::tempdir().unwrap();
    let home_dir = root.path().join("home/workflows");
    let alias_file = home_dir.join("alias.json");
    write(&alias_file, &valid_document("shared"));
    let store = FileWorkflowStore::new(vec![home_dir], root.path().join("state/runs"));

    store
        .delete("shared")
        .expect("home definitions are writable");

    assert!(!alias_file.exists());
    assert!(store.get("shared").unwrap().is_none());
}

#[test]
fn an_id_containing_a_dot_does_not_collide_on_its_temporary_file() {
    // The temp name is appended, not substituted for the extension, so
    // `a.b.json` and `a.json` cannot fight over one scratch path.
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    store
        .save(&parse_workflow(&valid_document("a.b"), "a.b").unwrap())
        .unwrap();
    store
        .save(&parse_workflow(&valid_document("a"), "a").unwrap())
        .unwrap();

    // Load order is the sorted filename order, so `a.b.json` precedes `a.json`.
    let ids: Vec<String> = store.list().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(ids, vec!["a.b", "a"], "both should survive");
}

#[test]
fn runs_are_listed_newest_first_and_scoped_to_their_workflow() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());

    store
        .record_run(&new_run_record("r1", "alpha", 100))
        .unwrap();
    store
        .record_run(&new_run_record("r2", "alpha", 300))
        .unwrap();
    store
        .record_run(&new_run_record("r3", "beta", 200))
        .unwrap();

    let alpha = store.list_runs("alpha").unwrap();
    let ids: Vec<&str> = alpha.iter().map(|r| r.id.as_str()).collect();

    assert_eq!(ids, vec!["r2", "r1"], "newest first");
    assert_eq!(store.list_runs("beta").unwrap().len(), 1);
    assert_eq!(store.list_runs("unknown").unwrap().len(), 0);
}

#[test]
fn a_run_record_survives_being_rewritten_as_it_settles() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut run = new_run_record("r1", "alpha", 100);
    store.record_run(&run).unwrap();

    run.status = RunStatus::PendingApproval;
    run.pending_approvals = vec!["review".into()];
    store.record_run(&run).unwrap();

    let loaded = require_run(&store, "r1").expect("found");
    assert_eq!(loaded.status, RunStatus::PendingApproval);
    assert_eq!(loaded.pending_approvals, vec!["review".to_string()]);
    assert!(!loaded.status.is_settled(), "an approval gate is resumable");
}

#[test]
fn asking_for_a_run_that_was_never_recorded_is_an_error_not_a_silent_none() {
    let root = tempfile::tempdir().unwrap();
    let err = require_run(&store_in(root.path()), "ghost").expect_err("no such run");
    assert!(matches!(err, WorkflowError::RunNotFound(_)), "got {err:?}");
}

#[test]
fn an_id_that_would_escape_the_workflow_directory_is_refused() {
    // A document's own `id` overrides whatever the caller asked for, and a
    // document may have been written by an agent — so this is the guard that
    // stops a save from writing outside the store with the daemon's rights.
    for hostile in [
        "../escape",
        "../../etc/authorized_keys",
        "sub/dir",
        "back\\slash",
        "..",
        ".",
        "   ",
        "/absolute",
    ] {
        assert!(
            super::file::safe_component(hostile).is_err(),
            "{hostile:?} should be refused"
        );
    }
    // Ordinary ids, including ones with dots, still work.
    for ordinary in ["sweep", "nightly-sweep", "a.b", "review_and_fix"] {
        assert!(
            super::file::safe_component(ordinary).is_ok(),
            "{ordinary:?} should be allowed"
        );
    }
}

#[test]
fn saving_a_workflow_whose_id_escapes_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut record = parse_workflow(&valid_document("ok"), "ok").unwrap();
    record.id = "../escaped".into();

    let err = store.save(&record).expect_err("must refuse");

    assert!(matches!(err, WorkflowError::Malformed(_)), "got {err:?}");
    assert!(
        !root.path().join("escaped.json").exists(),
        "nothing may be written outside the workflow directory"
    );
}

#[test]
fn a_run_id_that_escapes_is_refused_too() {
    // Run ids arrive on task frames from peers, so they are no more trusted
    // than a workflow id.
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());

    let err = store
        .record_run(&new_run_record("../escaped", "alpha", 1))
        .expect_err("must refuse");

    assert!(matches!(err, WorkflowError::Malformed(_)), "got {err:?}");
    assert!(!root.path().join("escaped.json").exists());
}

#[test]
fn a_workflow_saved_for_the_first_time_has_no_history_to_go_back_to() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());

    store
        .save(&parse_workflow(&valid_document("greet"), "greet").unwrap())
        .unwrap();

    // Nothing was replaced, so nothing was superseded.
    assert!(store.list_revisions("greet").unwrap().is_empty());
    assert!(undo_last(&store, "greet").unwrap().is_none());
}

#[test]
fn saving_over_a_workflow_snapshots_the_version_it_replaced() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut record = parse_workflow(&valid_document("greet"), "greet").unwrap();
    store.save(&record).unwrap();

    record.description = "rewritten by the copilot".into();
    store.save(&record).unwrap();

    let history = store.list_revisions("greet").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].record.description, "says hello");
}

#[test]
fn undo_restores_the_previous_version_and_is_itself_undoable() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut record = parse_workflow(&valid_document("greet"), "greet").unwrap();
    store.save(&record).unwrap();
    record.description = "rewritten by the copilot".into();
    store.save(&record).unwrap();

    let (revision, restored) = undo_last(&store, "greet").unwrap().expect("history");

    assert_eq!(restored.description, "says hello");
    assert_eq!(
        store.get("greet").unwrap().unwrap().description,
        "says hello"
    );
    // The rollback went through `save`, so the version it replaced was
    // snapshotted too: pressing undo twice returns to where you started.
    let history = store.list_revisions("greet").unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].record.description, "rewritten by the copilot");
    assert_ne!(history[0].id, revision.id);
}

#[test]
fn rolling_back_to_a_named_revision_restores_exactly_that_one() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut record = parse_workflow(&valid_document("greet"), "greet").unwrap();
    for description in ["first", "second", "third"] {
        record.description = description.into();
        store.save(&record).unwrap();
    }

    // Three saves, but the first replaced nothing — so history holds the two
    // versions that were superseded, newest first, and the last entry is the
    // oldest of those.
    let history = store.list_revisions("greet").unwrap();
    assert_eq!(history.len(), 2);
    let oldest = history.last().expect("history");
    let restored = rollback(&store, "greet", &oldest.id).unwrap();

    assert_eq!(restored.description, "first");
}

#[test]
fn rolling_back_to_a_revision_that_does_not_exist_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    store
        .save(&parse_workflow(&valid_document("greet"), "greet").unwrap())
        .unwrap();

    let err = rollback(&store, "greet", "no-such-revision").expect_err("must refuse");

    assert!(matches!(err, WorkflowError::Malformed(_)), "got {err:?}");
}

#[test]
fn deleting_a_workflow_leaves_it_recoverable() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    store
        .save(&parse_workflow(&valid_document("greet"), "greet").unwrap())
        .unwrap();

    store.delete("greet").unwrap();

    // A delete has nothing left to diff against, which is exactly why it is
    // snapshotted: without this it is the one edit that cannot be taken back.
    assert!(store.get("greet").unwrap().is_none());
    let history = store.list_revisions("greet").unwrap();
    assert_eq!(history.len(), 1);
    rollback(&store, "greet", &history[0].id).unwrap();
    assert!(store.get("greet").unwrap().is_some());
}

#[test]
fn history_does_not_show_up_in_the_workflow_listing() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut record = parse_workflow(&valid_document("greet"), "greet").unwrap();
    store.save(&record).unwrap();
    record.description = "again".into();
    store.save(&record).unwrap();

    // Snapshots sit in a subdirectory of the definition directory; a load that
    // wandered into it would show every past version as a workflow of its own.
    assert_eq!(store.list().unwrap().len(), 1);
    assert!(store.load().errors.is_empty());
}

#[test]
fn shadowing_a_project_default_with_a_home_workflow_snapshots_what_it_shadowed() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let project = root.path().join("project");
    write(&project.join("greet.json"), &valid_document("greet"));
    let store = FileWorkflowStore::new(vec![project, home], root.path().join("runs"));

    // The first home-level save writes a file that did not exist, so nothing
    // in the write directory is overwritten — but the operator *does* see the
    // graph change, because the home copy now shadows the project default.
    let mut record = require(&store, "greet").unwrap();
    record.description = "edited in this project".into();
    store.save(&record).unwrap();

    let history = store.list_revisions("greet").unwrap();
    assert_eq!(history.len(), 1, "the shadowed version must be recoverable");
    assert_eq!(history[0].record.description, "says hello");
    rollback(&store, "greet", &history[0].id).unwrap();
    assert_eq!(
        store.get("greet").unwrap().unwrap().description,
        "says hello"
    );
}
