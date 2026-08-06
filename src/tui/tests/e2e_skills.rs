//! End-to-end coverage for `medulla skills` — the generated-skill installer as
//! an operator actually invokes it.
//!
//! Every assertion here is about the filesystem or the exit status, because
//! those are the two things this command exists to affect and the two things a
//! unit test of the SDK cannot pin: the command layer is where flags become
//! paths, and a flag that resolves to the wrong path writes a real file into a
//! real checkout. The suite is offline and deterministic — a scratch Medulla
//! home holding hand-written workflow documents, a scratch install root, and a
//! working directory that is neither.

#![cfg(feature = "workflows")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// A scratch machine: a Medulla home with workflows in it, a working directory
/// the operator is "standing in", and an install root with a `.claude` in it.
///
/// The three are deliberately distinct directories. Most of what went wrong in
/// this command was one of them standing in for another.
struct Workspace {
    dir: TempDir,
}

impl Workspace {
    /// A workspace whose store holds one enabled workflow per id.
    fn new(ids: &[&str]) -> Self {
        let workspace = Workspace {
            dir: TempDir::new().expect("a scratch directory"),
        };
        std::fs::create_dir_all(workspace.workflows_dir()).unwrap();
        std::fs::create_dir_all(workspace.cwd()).unwrap();
        // The harness directory is what `default_targets` reads as "this
        // operator uses Claude Code"; without it an install has nowhere to go.
        std::fs::create_dir_all(workspace.root().join(".claude")).unwrap();
        std::fs::create_dir_all(workspace.fake_home()).unwrap();
        for id in ids {
            workspace.add_workflow(id);
        }
        workspace
    }

    /// The root `MEDULLA_HOME` names — the directory that holds accounts.
    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    /// Where a `HOME`-scoped install would land, kept away from the real one.
    fn fake_home(&self) -> PathBuf {
        self.dir.path().join("fake-home")
    }

    /// The account's workflow documents.
    fn workflows_dir(&self) -> PathBuf {
        self.home().join(ACCOUNT).join("workflows")
    }

    /// The directory commands are run from.
    fn cwd(&self) -> PathBuf {
        self.dir.path().join("cwd")
    }

    /// The `--dir` install root.
    fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    /// Write the smallest valid workflow document that still renders a skill.
    fn add_workflow(&self, id: &str) {
        let document = serde_json::json!({
            "id": id,
            "name": id,
            "description": format!("Do the {id}."),
            "enabled": true,
            "nodes": [{
                "id": "start",
                "kind": "trigger",
                "name": "Start",
                "config": { "trigger_kind": "manual" },
            }],
            "edges": [],
        });
        std::fs::write(
            self.workflows_dir().join(format!("{id}.json")),
            serde_json::to_string_pretty(&document).unwrap(),
        )
        .unwrap();
    }

    /// Run `medulla skills …` from [`cwd`](Self::cwd) against this scratch home.
    fn skills(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_medulla"));
        command
            .arg("skills")
            .args(args)
            .current_dir(self.cwd())
            .env("MEDULLA_HOME", self.home())
            // Pinned so the account directory does not depend on whatever the
            // developer happens to be signed in as.
            .env("MEDULLA_USER", ACCOUNT)
            .env("HOME", self.fake_home())
            .env_remove("MEDULLA_TOKEN")
            .env_remove("MEDULLA_BACKEND_URL")
            .env_remove("OPENROUTER_API_KEY");
        command.output().expect("the medulla binary should run")
    }

    /// Run with `--dir <root>` appended, the common shape of these tests.
    fn skills_in_root(&self, args: &[&str]) -> Output {
        let root = self.root();
        let mut all: Vec<&str> = args.to_vec();
        all.push("--dir");
        all.push(root.to_str().expect("a UTF-8 scratch path"));
        self.skills(&all)
    }

    /// The `SKILL.md` a workflow installs to under the scratch root.
    fn skill_path(&self, id: &str) -> PathBuf {
        self.root()
            .join(".claude")
            .join("skills")
            .join(format!("medulla-{id}"))
            .join("SKILL.md")
    }
}

/// The account directory every run in this suite uses.
const ACCOUNT: &str = "e2e";

/// Stdout as a string, for assertions and failure messages.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Stderr as a string.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Parse a `--json` run's document, failing loudly with the output if it is not
/// JSON — which is itself the bug worth hearing about.
fn json(output: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|err| panic!("stdout was not JSON ({err}): {}", stdout(output)))
}

/// The `(workflowId, action)` pairs of a report, in report order.
fn actions(document: &serde_json::Value) -> Vec<(String, String)> {
    document["files"]
        .as_array()
        .expect("a files array")
        .iter()
        .map(|file| {
            (
                file["workflowId"].as_str().unwrap_or_default().to_string(),
                file["action"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every file under `dir`, relative and sorted — the whole written tree.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = prefix.join(entry.file_name());
            if path.is_dir() {
                walk(&path, &relative, out);
            } else {
                out.push(relative.to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out);
    out.sort();
    out
}

#[test]
fn install_writes_the_skill_tree_and_a_repeat_changes_nothing() {
    let workspace = Workspace::new(&["babysit", "sweep"]);

    let first = workspace.skills_in_root(&["install", "--harness", "claude", "--with-commands"]);
    assert!(first.status.success(), "{}", stderr(&first));

    assert_eq!(
        tree(&workspace.root()),
        vec![
            ".claude/commands/medulla-babysit.md".to_string(),
            ".claude/commands/medulla-sweep.md".to_string(),
            ".claude/skills/medulla-babysit/SKILL.md".to_string(),
            ".claude/skills/medulla-sweep/SKILL.md".to_string(),
        ],
    );
    let body = std::fs::read_to_string(workspace.skill_path("babysit")).unwrap();
    assert!(
        body.starts_with("---\n# medulla:managed workflow=babysit"),
        "the managed marker is what every later run reads, and it sits inside the \
         frontmatter so the harness still parses it: {body}"
    );
    assert!(
        body.contains("mcp__medulla__workflow_run"),
        "a skill that names no tool triggers nothing: {body}"
    );

    // Idempotence is the whole point of the marker: a second install must not
    // rewrite a byte, or every `medulla skills sync` would churn the checkout.
    let second = workspace.skills_in_root(&[
        "install",
        "--harness",
        "claude",
        "--with-commands",
        "--json",
    ]);
    assert!(second.status.success(), "{}", stderr(&second));
    let report = json(&second);
    assert!(
        actions(&report)
            .iter()
            .all(|(_, action)| action == "unchanged"),
        "{report:#}"
    );
}

#[test]
fn a_dry_run_reports_the_same_outcome_and_writes_nothing() {
    let workspace = Workspace::new(&["babysit"]);

    let output =
        workspace.skills_in_root(&["install", "--harness", "claude", "--dry-run", "--json"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        actions(&json(&output)),
        vec![("babysit".to_string(), "created".to_string())],
    );
    // Not even the parent directories: a dry run leaves no trace at all.
    assert!(
        tree(&workspace.root()).is_empty(),
        "a dry run wrote {:?}",
        tree(&workspace.root()),
    );
}

#[test]
fn project_scope_with_an_explicit_dir_keeps_the_mcp_file_out_of_the_working_directory() {
    // Finding 3: `--dir` overrides `--scope`, so the registration has to follow
    // the root. It used to follow the *working directory*, which meant an
    // install aimed at /tmp dropped an unrequested `.mcp.json` into whichever
    // repository the operator happened to be standing in.
    let workspace = Workspace::new(&["babysit"]);

    let output = workspace.skills_in_root(&[
        "install",
        "--harness",
        "claude",
        "--scope",
        "project",
        "--with-mcp",
    ]);

    assert!(output.status.success(), "{}", stderr(&output));
    let registered = workspace.root().join(".mcp.json");
    assert!(
        registered.is_file(),
        "the registration belongs under --dir: {}",
        stdout(&output)
    );
    assert!(
        !workspace.cwd().join(".mcp.json").exists(),
        "an unrequested .mcp.json landed in the working directory: {:?}",
        tree(&workspace.cwd()),
    );
    let document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&registered).unwrap()).unwrap();
    assert_eq!(document["mcpServers"]["medulla"]["args"][0], "mcp");
    assert_eq!(
        document["mcpServers"]["medulla"]["env"]["MEDULLA_WORKFLOW_TOOLS"],
        "run",
    );
}

#[test]
fn a_collision_leaves_the_file_alone_and_exits_non_zero() {
    // Finding 5: the report already said `skippedUnmanaged`, but the command
    // exited 0, so a CI wrapper could not tell that a named workflow had not
    // been installed at all.
    let workspace = Workspace::new(&["babysit", "sweep"]);
    let squatter = workspace.skill_path("babysit");
    std::fs::create_dir_all(squatter.parent().unwrap()).unwrap();
    std::fs::write(&squatter, "hand-written, not ours\n").unwrap();

    let output = workspace.skills_in_root(&["install", "--harness", "claude"]);

    assert!(
        !output.status.success(),
        "a collision must fail the command: {}",
        stdout(&output)
    );
    // The report still reaches stdout: the status says something is wrong, the
    // report says which file.
    assert!(
        stdout(&output).contains("skippedUnmanaged"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("medulla-sweep"),
        "the workflows that did install are still reported: {}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("not every skill was installed"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        std::fs::read_to_string(&squatter).unwrap(),
        "hand-written, not ours\n",
        "the operator's file was overwritten",
    );

    // The JSON shape is unchanged by the failure — `collisions` is how a
    // machine reads the same fact.
    let as_json = workspace.skills_in_root(&["install", "--harness", "claude", "--json"]);
    assert!(!as_json.status.success());
    assert_eq!(json(&as_json)["collisions"], true);
}

#[test]
fn sync_with_named_ids_touches_only_those_workflows() {
    // Finding 8: the ids were parsed and then dropped, so `sync babysit` synced
    // the entire store.
    let workspace = Workspace::new(&["babysit", "sweep"]);

    let output = workspace.skills_in_root(&["sync", "babysit", "--harness", "claude"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        tree(&workspace.root()),
        vec![".claude/skills/medulla-babysit/SKILL.md".to_string()],
        "sync touched a workflow the operator did not name",
    );
    assert_eq!(
        actions(&json(&workspace.skills_in_root(&[
            "sync",
            "babysit",
            "--harness",
            "claude",
            "--json",
        ]))),
        vec![("babysit".to_string(), "unchanged".to_string())],
    );

    // With no ids it is still the whole store.
    let everything = workspace.skills_in_root(&["sync", "--harness", "claude"]);
    assert!(everything.status.success(), "{}", stderr(&everything));
    assert!(workspace.skill_path("sweep").is_file());
}

#[test]
fn prune_with_named_ids_is_refused_before_anything_is_deleted() {
    // The combination has no honest reading: pruning against a one-id keep set
    // would delete the skill for every workflow the operator did not name.
    let workspace = Workspace::new(&["babysit", "sweep"]);
    assert!(workspace
        .skills_in_root(&["install", "--harness", "claude"])
        .status
        .success());

    let output = workspace.skills_in_root(&["sync", "babysit", "--prune", "--harness", "claude"]);

    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("--prune"), "{}", stderr(&output));
    assert!(
        workspace.skill_path("sweep").is_file(),
        "the unnamed workflow's skill was pruned anyway",
    );
}

#[test]
fn a_bare_uninstall_lists_what_it_would_remove_and_refuses() {
    // Finding 9: removing every managed skill was the *default*, one typo away
    // from `uninstall babysit` and not recoverable from the output.
    let workspace = Workspace::new(&["babysit", "sweep"]);
    assert!(workspace
        .skills_in_root(&["install", "--harness", "claude"])
        .status
        .success());

    let refused = workspace.skills_in_root(&["uninstall", "--harness", "claude"]);

    assert!(
        !refused.status.success(),
        "a blanket uninstall must not succeed silently: {}",
        stdout(&refused)
    );
    assert!(
        workspace.skill_path("babysit").is_file() && workspace.skill_path("sweep").is_file(),
        "the refusal still deleted something",
    );
    // What it would have removed is on stdout, so `--all` is an informed choice.
    assert!(stdout(&refused).contains("babysit"), "{}", stdout(&refused));
    assert!(stdout(&refused).contains("sweep"), "{}", stdout(&refused));
    assert!(stderr(&refused).contains("--all"), "{}", stderr(&refused));

    // A named id needs no ceremony, and removes only itself.
    let one = workspace.skills_in_root(&["uninstall", "babysit", "--harness", "claude"]);
    assert!(one.status.success(), "{}", stderr(&one));
    assert!(!workspace.skill_path("babysit").exists());
    assert!(workspace.skill_path("sweep").is_file());

    // `--all` is the explicit form of what used to be the default.
    let all = workspace.skills_in_root(&["uninstall", "--all", "--harness", "claude"]);
    assert!(all.status.success(), "{}", stderr(&all));
    assert!(
        tree(&workspace.root()).is_empty(),
        "{:?}",
        tree(&workspace.root())
    );
}
