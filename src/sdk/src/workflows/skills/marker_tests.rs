//! Unit tests for the managed-marker discipline when the inputs are not
//! well-behaved: ids the store accepts but a filename cannot express, two
//! workflows whose slugs land on one path, and files that are not text at all.
//!
//! Kept apart from [`super::tests`] because these are the failure modes that
//! destroy data rather than merely render badly — a marker that does not
//! round-trip makes Medulla disown a file it wrote and then prune it.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use crate::workflows::{ops, FileWorkflowStore, WorkflowStore, WorkflowSummary};

use super::render::parse_marker;
use super::*;

/// Every id shape the store accepts that the old raw marker could not express.
///
/// `store/file/paths.rs::safe_component` rejects only an empty id, `.`, `..`,
/// and ids containing a path separator or NUL — so all of these are ids an
/// operator can really author, and each one broke the marker differently:
/// whitespace split the field list, a newline moved `rev` off the first line.
const AWKWARD_IDS: [&str; 6] = [
    "nightly sweep",
    "nightly\nsweep",
    "say \"hello\"",
    "100% coverage",
    "révision-café",
    "plain-old-id",
];

/// A listing view with the fields skill rendering actually reads.
fn summary(id: &str) -> WorkflowSummary {
    WorkflowSummary {
        id: id.to_string(),
        name: id.to_string(),
        description: "Do the thing.".to_string(),
        enabled: true,
        node_count: 3,
        trigger_kind: Some("manual".to_string()),
        inputs: vec![],
    }
}

/// Install options rooted at a scratch directory, writing skills and commands.
fn opts(root: &Path) -> InstallOptions {
    InstallOptions {
        targets: vec![SkillTarget::Claude],
        scope: SkillScope::User,
        root: root.to_path_buf(),
        with_commands: true,
        dry_run: false,
    }
}

/// A runnable one-agent workflow document, as authored through `ops`.
fn document(id: &str) -> String {
    json!({
        "id": id,
        "name": id,
        "description": "Sweep the estate nightly",
        "inputs": [],
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "do it" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string()
}

#[test]
fn every_id_the_store_accepts_round_trips_through_its_marker() {
    for id in AWKWARD_IDS {
        let skill = render(&summary(id));
        let marker = skill.body.lines().next().expect("a marker line");

        // The whole marker stays on line one, whatever the id contains.
        assert!(marker.ends_with("-->"), "{id:?} broke the marker: {marker}");
        assert!(!marker["<!-- medulla:managed ".len()..].contains(['\n', '\r']));

        let (decoded, rev) = parse_marker(&skill.body).expect("our own marker parses");
        assert_eq!(decoded, id, "id did not survive the marker");
        assert_eq!(rev, skill.rev);

        // The command variant seals its id the same way.
        let command = render_command(&skill, &summary(id));
        assert_eq!(parse_marker(&command).expect("marker").0, id);
    }
}

#[test]
fn a_marker_we_could_not_have_written_is_not_ours() {
    // Two ids that differ only in what percent-encoding hides must not collapse
    // onto one marker.
    let literal = render(&summary("a b")).body;
    let escaped = render(&summary("a%20b")).body;
    assert_ne!(
        parse_marker(&literal).unwrap().0,
        parse_marker(&escaped).unwrap().0
    );

    let rejected = [
        // The pre-encoding spelling: an id with a raw space, which used to
        // parse as the truncated id `nightly`.
        "<!-- medulla:managed workflow=nightly sweep rev=ab12 -->\n",
        // Escapes that cannot be completed or decoded.
        "<!-- medulla:managed workflow=a%2 rev=ab12 -->\n",
        "<!-- medulla:managed workflow=a%zz rev=ab12 -->\n",
        "<!-- medulla:managed workflow=a%FF rev=ab12 -->\n",
        // A literal byte outside the encoder's alphabet.
        "<!-- medulla:managed workflow=a~b rev=ab12 -->\n",
        // Empty, duplicated, or missing fields.
        "<!-- medulla:managed workflow= rev=ab12 -->\n",
        "<!-- medulla:managed workflow=a workflow=b rev=ab12 -->\n",
        "<!-- medulla:managed workflow=a rev=ab12 rev=cd34 -->\n",
        "<!-- medulla:managed workflow=a -->\n",
        "<!-- medulla:managed rev=ab12 -->\n",
        "<!-- medulla:managed workflow=a rev=nothex -->\n",
        // Not a marker at all.
        "hand written, do not touch\n",
    ];
    for file in rejected {
        assert!(parse_marker(file).is_none(), "should be rejected: {file}");
    }

    // An unknown field from a later release still identifies its workflow.
    assert_eq!(
        parse_marker("<!-- medulla:managed workflow=a rev=ab12 flavour=x -->\n").unwrap(),
        ("a".to_string(), "ab12".to_string())
    );
}

#[test]
fn an_id_with_whitespace_installs_once_and_survives_a_prune() {
    // Authored through the real store, so the test also pins that this id is
    // one an operator can actually save.
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    ops::create(&store, &document("nightly sweep"), "nightly sweep")
        .expect("the store accepts an id with a space");
    let workflows = store.list().expect("listing");
    assert_eq!(workflows[0].id, "nightly sweep");

    let home = tempfile::tempdir().unwrap();
    let options = opts(home.path());
    let path = skill_path(SkillTarget::Claude, home.path(), "medulla-nightly-sweep");

    let first = install(&workflows, &options).unwrap();
    assert_eq!(first.count(FileAction::Created), 2, "{first:?}");
    let written = fs::read_to_string(&path).unwrap();

    // Re-installing recognises the file as ours rather than as a squatter's.
    let second = install(&workflows, &options).unwrap();
    assert_eq!(second.count(FileAction::Unchanged), 2, "{second:?}");
    assert!(!second.has_collisions());

    // And the prune keeps the live workflow's skill instead of deleting it.
    let pruned = sync(&workflows, &options, true).unwrap();
    assert_eq!(pruned.count(FileAction::Removed), 0, "{pruned:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), written);

    let found = installed(&options).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].workflow_id, "nightly sweep");
}

#[test]
fn two_workflows_that_slugify_alike_collide_instead_of_shadowing() {
    let home = tempfile::tempdir().unwrap();
    let options = opts(home.path());
    let workflows = vec![summary("deploy.prod"), summary("deploy-prod")];

    let report = install(&workflows, &options).unwrap();

    // The first claim wins; the second is named, not silently dropped.
    assert_eq!(report.count(FileAction::Created), 2, "{report:?}");
    assert_eq!(report.count(FileAction::SlugCollision), 2, "{report:?}");
    assert_eq!(report.count(FileAction::SkippedUnmanaged), 0, "{report:?}");
    assert!(report.has_collisions());
    assert!(report
        .files
        .iter()
        .filter(|file| file.workflow_id == "deploy-prod")
        .all(|file| file.action == FileAction::SlugCollision));

    let path = skill_path(SkillTarget::Claude, home.path(), "medulla-deploy-prod");
    assert!(fs::read_to_string(path)
        .unwrap()
        .contains("workflow=deploy.prod"));
}

#[test]
fn a_marker_for_another_workflow_is_a_collision_not_an_unmanaged_file() {
    let home = tempfile::tempdir().unwrap();
    let options = opts(home.path());
    install(&[summary("deploy.prod")], &options).unwrap();

    // A second run, a later day: only the other workflow is installed.
    let report = install(&[summary("deploy-prod")], &options).unwrap();

    assert_eq!(report.count(FileAction::SlugCollision), 2, "{report:?}");
    assert_eq!(report.count(FileAction::SkippedUnmanaged), 0, "{report:?}");
    assert!(report.has_collisions());
}

#[test]
fn a_dry_run_reaches_the_same_verdicts_as_the_real_run() {
    let home = tempfile::tempdir().unwrap();
    let options = opts(home.path());

    // A tree with one of every awkward case: a hand-written squatter, a file
    // Medulla generated for a workflow that is no longer in the list, and two
    // workflows that want the same path as each other.
    let squatted = skill_path(SkillTarget::Claude, home.path(), "medulla-babysit");
    fs::create_dir_all(squatted.parent().unwrap()).unwrap();
    fs::write(&squatted, "hand written, do not touch\n").unwrap();
    let stale = skill_path(SkillTarget::Claude, home.path(), "medulla-audit");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, render(&summary("retired")).body).unwrap();

    let workflows = vec![
        summary("babysit"),
        summary("audit"),
        summary("deploy.prod"),
        summary("deploy-prod"),
    ];

    let mut dry_options = options.clone();
    dry_options.dry_run = true;
    let dry = install(&workflows, &dry_options).unwrap();
    let real = install(&workflows, &options).unwrap();

    assert_eq!(dry, real, "dry run and real run disagreed");
    // Not vacuous: every verdict the fixture was built to provoke is present.
    assert_eq!(dry.count(FileAction::SkippedUnmanaged), 1, "{dry:?}");
    assert_eq!(dry.count(FileAction::SlugCollision), 3, "{dry:?}");
    assert_eq!(dry.count(FileAction::Created), 4, "{dry:?}");
    assert_eq!(
        fs::read_to_string(&squatted).unwrap(),
        "hand written, do not touch\n"
    );
}

#[test]
fn a_file_that_is_not_utf8_is_left_alone_without_aborting_the_run() {
    let home = tempfile::tempdir().unwrap();
    let options = opts(home.path());
    install(&[summary("babysit")], &options).unwrap();

    // Invalid UTF-8 in both scanned directories: a lone continuation byte and a
    // truncated multi-byte sequence.
    let rogue_skill = skill_path(SkillTarget::Claude, home.path(), "medulla-rogue");
    fs::create_dir_all(rogue_skill.parent().unwrap()).unwrap();
    fs::write(&rogue_skill, [0xff, 0xfe, 0x00, 0x9f]).unwrap();
    let rogue_command = home.path().join(".claude/commands/medulla-rogue.md");
    fs::write(&rogue_command, [0xe2, 0x28, 0xa1]).unwrap();

    // Scanning, syncing, and uninstalling all complete rather than failing with
    // `InvalidData`, and the unreadable files keep their bytes.
    let found = installed(&options).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].workflow_id, "babysit");

    let synced = sync(&[summary("babysit")], &options, true).unwrap();
    assert_eq!(synced.count(FileAction::Removed), 0, "{synced:?}");
    assert_eq!(fs::read(&rogue_skill).unwrap(), [0xff, 0xfe, 0x00, 0x9f]);
    assert_eq!(fs::read(&rogue_command).unwrap(), [0xe2, 0x28, 0xa1]);

    let removed = uninstall(&["babysit".to_string()], &options).unwrap();
    assert_eq!(removed.count(FileAction::Removed), 2, "{removed:?}");

    // And an install onto one of those paths reports it as someone else's file
    // rather than overwriting bytes it cannot read.
    let report = install(&[summary("rogue")], &options).unwrap();
    assert_eq!(report.count(FileAction::SkippedUnmanaged), 2, "{report:?}");
    assert_eq!(fs::read(&rogue_skill).unwrap(), [0xff, 0xfe, 0x00, 0x9f]);
}
