//! Unit tests for the agent-template catalog and its on-disk store: the shape
//! every default role must have, what the store does with good and bad files,
//! and the install's never-overwrite rule.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::defaults::{default_template_files, default_templates};
use super::install::{install_default_templates, InstallOutcome};
use super::store::{load_templates, merge_templates, template_dirs};
use crate::runtime::fleet::{AgentTemplate, CapacitySnapshot};

/// A scratch directory that removes itself, so a test can write real files
/// without leaving any behind.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("medulla-agents-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, body: &str) {
        std::fs::write(self.0.join(name), body).expect("write");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A minimal declared template, for merge collisions.
fn declared(id: &str, description: &str) -> AgentTemplate {
    AgentTemplate {
        id: id.into(),
        name: None,
        description: description.into(),
        instructions: None,
        tools: None,
        model: None,
        effort: None,
        params: Default::default(),
        tags: Vec::new(),
        metadata: Default::default(),
        harnesses: Default::default(),
    }
}

#[test]
fn the_catalog_declares_exactly_the_promised_roles() {
    let templates = default_templates();
    let ids: Vec<&str> = templates.iter().map(|t| t.id.as_str()).collect();
    // The ids are the contract: a config, a workspace allowlist, or a running
    // agent may name any of them, so renaming or dropping one is a breaking
    // change and has to be a deliberate edit here.
    assert_eq!(
        ids,
        [
            "plan-writer",
            "implementer",
            "test-writer",
            "code-reviewer",
            "debugger",
            "verifier",
            "doc-writer",
            "refactorer",
            "merge-resolver",
            "pr-manager",
            "triager",
            "repo-orchestrator",
        ]
    );

    for template in &templates {
        assert!(template.name.is_some(), "{} has no name", template.id);
        assert!(
            !template.description.trim().is_empty(),
            "{} has no description",
            template.id
        );
        // The description is the catalog line and the instructions are the
        // prompt surface: a role missing either is a row you cannot act on.
        assert!(
            template
                .instructions
                .as_deref()
                .is_some_and(|i| !i.trim().is_empty()),
            "{} has no instructions",
            template.id
        );
        assert!(
            template.tools.as_ref().is_some_and(|t| !t.is_empty()),
            "{} grants no tools",
            template.id
        );
        // An empty override map means "any harness kind"; a non-empty one is an
        // allowlist, which a default role must never impose.
        assert!(
            template.harnesses.is_empty(),
            "{} restricts harness kinds",
            template.id
        );
        assert!(
            template.tags.contains(&"code".to_string()),
            "{} is not tagged as coding work",
            template.id
        );
    }
}

#[test]
fn merging_fills_gaps_and_never_replaces_a_declared_template() {
    let capacity = CapacitySnapshot {
        templates: vec![declared("code-reviewer", "ours, not theirs")],
        ..Default::default()
    };
    let out = merge_templates(&capacity, &default_templates());

    let reviewers: Vec<&AgentTemplate> = out
        .templates
        .iter()
        .filter(|t| t.id == "code-reviewer")
        .collect();
    assert_eq!(reviewers.len(), 1, "one row per id");
    assert_eq!(reviewers[0].description, "ours, not theirs");
    assert_eq!(out.templates[0].id, "code-reviewer", "declared reads first");
    assert_eq!(out.templates.len(), default_templates().len());
    // Only the catalog is touched — the containment chain is left as declared.
    assert!(out.hosts.is_empty() && out.harnesses.is_empty());
    assert_eq!(merge_templates(&out, &default_templates()), out);
}

#[test]
fn the_store_reads_a_directory_of_toml_files() {
    let dir = TempDir::new("read");
    dir.write(
        "reviewer.toml",
        r#"
name = "Reviewer"
description = "Reads a diff."
tools = ["read"]
instructions = '''
Read the diff and say what breaks.
'''
"#,
    );
    // A file that names its own id keeps it; one that does not is named by the
    // filename, so the smallest useful file is a name and some instructions.
    dir.write(
        "second.toml",
        r#"
id = "explicit-id"
description = "Named itself."
"#,
    );
    dir.write("notes.md", "not a template");

    let store = load_templates(&[dir.path().to_path_buf()]);
    assert!(store.errors.is_empty(), "{:?}", store.errors);
    let ids: Vec<&str> = store.templates.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["reviewer", "explicit-id"], "sorted by filename");
    let reviewer = &store.templates[0];
    assert_eq!(reviewer.name.as_deref(), Some("Reviewer"));
    assert_eq!(
        reviewer.instructions.as_deref(),
        Some("Read the diff and say what breaks.\n")
    );
    assert_eq!(store.dirs, vec![dir.path().to_path_buf()]);
}

#[test]
fn a_document_with_the_wrong_shape_is_rejected_with_its_error() {
    // Right TOML, wrong types: the serde message is what reaches the operator.
    let err = super::store::parse_template("tools = 3\n", "fallback").unwrap_err();
    assert!(err.contains("expected a sequence"), "{err}");
}

#[test]
fn a_broken_file_costs_only_itself() {
    let dir = TempDir::new("broken");
    dir.write("good.toml", "description = \"fine\"\n");
    dir.write("bad.toml", "this is not = = toml\n");

    let store = load_templates(&[dir.path().to_path_buf()]);
    assert_eq!(store.templates.len(), 1);
    assert_eq!(store.templates[0].id, "good");
    assert_eq!(store.errors.len(), 1);
    assert!(store.errors[0].contains("bad.toml"), "{:?}", store.errors);
}

#[test]
fn a_missing_directory_is_empty_rather_than_an_error() {
    let store = load_templates(&[PathBuf::from("/nonexistent/medulla/agents")]);
    assert!(store.is_empty());
    assert!(store.errors.is_empty());
    assert!(store.dirs.is_empty());
}

#[test]
fn a_later_directory_overrides_an_earlier_one_in_place() {
    let home = TempDir::new("home");
    let project = TempDir::new("project");
    home.write("reviewer.toml", "description = \"user-global\"\n");
    home.write("planner.toml", "description = \"user-global planner\"\n");
    project.write("reviewer.toml", "description = \"project-local\"\n");

    let store = load_templates(&[home.path().to_path_buf(), project.path().to_path_buf()]);
    let ids: Vec<&str> = store.templates.iter().map(|t| t.id.as_str()).collect();
    // The override keeps the position of what it overrode, so a project-local
    // edit does not reshuffle the catalog.
    assert_eq!(ids, ["planner", "reviewer"]);
    assert_eq!(store.templates[1].description, "project-local");
}

#[test]
fn install_writes_editable_files_and_never_overwrites_them() {
    let dir = TempDir::new("install");
    let store_dir = dir.path().join("agents");

    let first = install_default_templates(&store_dir).expect("install");
    assert_eq!(first.written.len(), default_templates().len());
    assert!(first.skipped.is_empty());
    assert!(first.summary().contains("Installed"));

    // What was written is what the store reads back.
    let store = load_templates(std::slice::from_ref(&store_dir));
    assert!(store.errors.is_empty(), "{:?}", store.errors);
    let mut installed: Vec<String> = store.templates.iter().map(|t| t.id.clone()).collect();
    let mut expected: Vec<String> = default_templates().iter().map(|t| t.id.clone()).collect();
    installed.sort();
    expected.sort();
    assert_eq!(installed, expected);
    let reviewer = store
        .templates
        .iter()
        .find(|t| t.id == "code-reviewer")
        .expect("code-reviewer");
    let original = default_templates()
        .into_iter()
        .find(|t| t.id == "code-reviewer")
        .expect("code-reviewer");
    assert_eq!(reviewer.tools, original.tools);
    assert_eq!(reviewer.effort, original.effort);
    assert_eq!(
        reviewer.instructions.as_deref().map(str::trim),
        original.instructions.as_deref().map(str::trim)
    );
    // The bytes are the shipped document, comments and all — installing copies
    // it rather than re-serializing a parsed template.
    let text = std::fs::read_to_string(store_dir.join("code-reviewer.toml")).expect("read");
    let shipped = default_template_files()
        .iter()
        .find(|(name, _)| *name == "code-reviewer.toml")
        .expect("shipped document")
        .1;
    assert_eq!(text, shipped);
    assert!(text.contains("instructions = '''"), "{text}");
    assert!(text.starts_with("# A medulla agent template"), "{text}");

    // An edit survives a second install, which only reports what it skipped.
    std::fs::write(
        store_dir.join("code-reviewer.toml"),
        "description = \"mine\"\n",
    )
    .expect("edit");
    let second = install_default_templates(&store_dir).expect("reinstall");
    assert!(second.written.is_empty());
    assert_eq!(second.skipped.len(), default_templates().len());
    assert!(second.summary().contains("already installed"));
    let reread = load_templates(std::slice::from_ref(&store_dir));
    assert_eq!(
        reread
            .templates
            .iter()
            .find(|t| t.id == "code-reviewer")
            .map(|t| t.description.as_str()),
        Some("mine")
    );
}

#[test]
fn the_dirs_are_home_then_project_and_collapse_when_they_are_one() {
    let env: HashMap<String, String> = [("MEDULLA_HOME".to_string(), "/tmp/mh".to_string())]
        .into_iter()
        .collect();
    let dirs = template_dirs(&env, Path::new("/work/repo"));
    assert_eq!(
        dirs,
        vec![
            PathBuf::from("/tmp/mh/agents"),
            PathBuf::from("/work/repo/.medulla/agents"),
        ]
    );

    // `MEDULLA_DEV` puts the home at `./.medulla`, which *is* the project store.
    let dev: HashMap<String, String> = [("MEDULLA_DEV".to_string(), "1".to_string())]
        .into_iter()
        .collect();
    assert_eq!(
        template_dirs(&dev, Path::new(".")),
        vec![PathBuf::from(".medulla/agents")]
    );
}

#[test]
fn every_shipped_document_parses_and_names_its_own_id() {
    // The catalog is only as good as its documents: a typo in one would drop a
    // role silently, since parsing skips what it cannot read.
    let files = default_template_files();
    assert_eq!(
        files.len(),
        default_templates().len(),
        "a document failed to parse"
    );
    for (name, body) in files {
        let stem = name.strip_suffix(".toml").expect("a .toml document");
        let parsed = super::store::parse_template(body, "unused-fallback")
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        // Every shipped document names its id explicitly, so the filename and
        // the id can never drift apart.
        assert_eq!(parsed.id, stem, "{name} names a different id");
    }
}

#[test]
fn a_directory_that_cannot_be_read_is_reported_rather_than_skipped() {
    // A file where a directory is expected fails with something other than
    // NotFound, which is the branch that has to reach the operator.
    let dir = TempDir::new("not-a-dir");
    let path = dir.path().join("agents");
    std::fs::write(&path, "I am a file").expect("write");

    let store = load_templates(std::slice::from_ref(&path));
    assert!(store.is_empty());
    assert_eq!(store.errors.len(), 1);
    assert!(store.errors[0].contains("agents"), "{:?}", store.errors);
}

#[test]
fn install_reports_a_directory_it_cannot_create() {
    let dir = TempDir::new("blocked");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "not a directory").expect("write");

    let err = install_default_templates(&blocker.join("agents")).unwrap_err();
    assert!(
        matches!(
            err.kind(),
            std::io::ErrorKind::NotADirectory | std::io::ErrorKind::PermissionDenied
        ),
        "{err:?}"
    );
}

#[test]
fn the_install_summary_says_what_happened() {
    let mut outcome = InstallOutcome {
        dir: PathBuf::from("/tmp/agents"),
        ..Default::default()
    };
    assert!(outcome.summary().starts_with("No templates to install"));

    outcome.written = vec!["a.toml".into()];
    assert!(outcome.summary().contains("Installed 1 template into"));

    outcome.skipped = vec!["b.toml".into(), "c.toml".into()];
    assert!(outcome.summary().contains("(2 already there)"));

    outcome.written.clear();
    assert!(outcome.summary().contains("2 templates already installed"));
}
