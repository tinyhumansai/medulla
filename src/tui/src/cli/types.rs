//! CLI data model: the top-level [`Command`], the per-subcommand parsed-flag
//! structs ([`LoginArgs`], [`UpdateArgs`],
//! [`TuiArgs`]). The parsers that build these live in the sibling `parse`
//! module.

use medulla::auth::Provider;
use medulla::protocol::HarnessProvider;

/// The top-level subcommand selected from `argv[1..]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the interactive TUI (bare invocation, or with TUI flags).
    Tui,
    /// Run the non-interactive core-runtime driver: submit one instruction to
    /// the embedded core and stream the cycle's events as JSON lines.
    Run,
    /// Run the headless daemon; carries the remaining args.
    Daemon,
    /// `medulla daemon --tui` — the worker daemon with its operator screen.
    DaemonTui,
    Version,
    Help,
    Sessions,
    /// Log in to the backend (loopback OAuth or a one-time token).
    Login,
    /// Clear stored credentials.
    Logout,
    /// Launch a coding-agent CLI as a transparent host-link-bridged wrapper.
    Wrapper(HarnessProvider),
    /// Check for / install a newer release (`update [--check]`).
    Update,
    /// Author a `MEDULLA.md` workspace profile for a directory.
    Init,
    /// Manage the workspace registry (`add`/`list`/`remove`) — the directories
    /// the orchestrator knows about and can place work in.
    Workspace,
    /// Run the orchestrator hub: relay hosted-backend tasks to host-link
    /// workers over Signal DMs; carries the remaining args.
    Hub,
    /// Author, inspect, and run workflows (`list`/`get`/`create`/`apply-ops`/…).
    Workflow,
    /// Serve Medulla's tools over MCP on stdin/stdout.
    ///
    /// Not for a human to run. Medulla spawns this itself when it starts a
    /// harness, handing the child a socket path and a grant token in its
    /// environment; run by hand it serves the workflow tools and no fleet.
    Mcp,
    /// Generate harness-native skills that trigger saved workflows
    /// (`list`/`install`/`sync`/`uninstall`).
    Skills,
    /// Report one harness lifecycle event (`hook <Event>`).
    ///
    /// Not for a human to run either. Medulla installs this as a hook into the
    /// harnesses it launches; the harness runs it, pipes its native hook payload
    /// to it, and it files a one-line report on the control socket the same
    /// spawn was handed.
    Hook,
}

/// The `medulla skills` action.
///
/// Deliberately four verbs and no more: everything an operator does to a
/// generated skill is "show me", "put it there", "make it match the store
/// again", or "take it away".
#[cfg(feature = "workflows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillsAction {
    /// `list` — the managed skills currently on disk. The default, because it
    /// is the read-only answer to a half-remembered command.
    #[default]
    List,
    /// `install [<id>…]` — write skills for the named workflows, or all of them.
    Install,
    /// `sync` — reinstall every enabled workflow; with `--prune`, drop the rest.
    Sync,
    /// `uninstall [<id>…]` — remove managed skills.
    Uninstall,
}

/// Parsed `medulla skills` flags.
///
/// The struct is the whole contract between [`parse_skills_args`] and the
/// command: parsing resolves nothing about the filesystem, so every field here
/// is either a literal from the command line or a validated enum. Where the
/// root actually lands, and which harnesses exist on this machine, is decided
/// by the runner.
///
/// [`parse_skills_args`]: super::parse_skills_args
#[cfg(feature = "workflows")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillsArgs {
    /// The selected verb.
    pub action: SkillsAction,
    /// Workflow ids named after the verb. Empty means "whatever the verb's
    /// default set is" — every enabled workflow for `install` and `sync`, and,
    /// for `uninstall`, everything managed on disk *only* once [`all`] says so.
    ///
    /// [`all`]: SkillsArgs::all
    pub ids: Vec<String>,
    /// `--harness <a,b>` (repeatable, comma-joined), or `all`. `None` means the
    /// operator named none, and the runner should pick the harnesses whose
    /// directories already exist.
    pub targets: Option<Vec<medulla::workflows::skills::SkillTarget>>,
    /// `--scope user|project|managed`: whether the root is `$HOME`, the project,
    /// or Medulla's own root that harnesses it spawns are pointed at.
    pub scope: medulla::workflows::skills::SkillScope,
    /// `--dir <path>`: an explicit root, overriding whatever `--scope` would
    /// have resolved to. Relative paths are resolved against the cwd by the
    /// runner, not here.
    pub dir: Option<String>,
    /// `--with-mcp`: also register `medulla mcp` with each harness, without
    /// which a generated skill has no tool to call.
    pub with_mcp: bool,
    /// `--with-commands`: also emit the slash-command variant.
    pub with_commands: bool,
    /// `--tools run|full`: the tool surface a skill-triggered MCP session gets.
    /// Defaults to `run` — a trigger-only session has no business rewriting a
    /// graph.
    pub tools: String,
    /// `--dry-run`: report the identical outcome and write nothing.
    pub dry_run: bool,
    /// `--prune` (`sync` only): delete managed skills for workflows that are no
    /// longer enabled or no longer exist.
    pub prune: bool,
    /// `--all` (`uninstall` only): the explicit consent a blanket removal
    /// needs. Without it a bare `uninstall` lists what it would delete and
    /// fails, because "remove every managed skill" is one typo away from
    /// `uninstall babysit` and is not recoverable from the output. Ignored when
    /// ids are named — those are already an explicit choice.
    pub all: bool,
    /// `--json`: machine-readable output instead of the human summary.
    pub json: bool,
}

#[cfg(feature = "workflows")]
impl Default for SkillsArgs {
    fn default() -> Self {
        SkillsArgs {
            action: SkillsAction::List,
            ids: Vec::new(),
            targets: None,
            scope: medulla::workflows::skills::SkillScope::User,
            dir: None,
            with_mcp: false,
            with_commands: false,
            tools: "run".to_string(),
            dry_run: false,
            prune: false,
            all: false,
            json: false,
        }
    }
}

/// Parsed `medulla init` flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InitArgs {
    /// Directory to initialise. `None` means the current working directory.
    pub dir: Option<String>,
    /// Explicit `--config` path used to resolve the backend/model settings.
    /// `None` selects the layered config discovery.
    pub config: Option<String>,
    /// `--force`: overwrite an existing `MEDULLA.md`.
    pub force: bool,
    /// `--offline`: skip the model call and write the editable stub.
    pub offline: bool,
}

/// The `medulla workspace` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// `add [dir]` — author the profile *and* enrol the directory in the
    /// registry. `None` targets the current working directory.
    Add(Option<String>),
    /// `list` — show every registered workspace.
    List,
    /// `remove <dir|id>` — unregister a workspace, leaving its files alone.
    Remove(String),
}

/// Parsed `medulla workspace` flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceArgs {
    /// Explicit `--config` path — the file the registry is read from and written
    /// to. `None` selects the layered config discovery.
    pub config: Option<String>,
    /// `--harness <id>`: attach the workspace to this harness instead of the
    /// first declared one (`add` only).
    pub harness: Option<String>,
    /// `--force`: overwrite an existing `MEDULLA.md` (`add` only).
    pub force: bool,
    /// `--offline`: skip the model call and write the editable stub (`add` only).
    pub offline: bool,
    /// Emit JSON instead of human-readable output (`list` only).
    pub json: bool,
    /// The selected action.
    pub action: WorkspaceAction,
}

impl Default for WorkspaceArgs {
    fn default() -> Self {
        WorkspaceArgs {
            config: None,
            harness: None,
            force: false,
            offline: false,
            json: false,
            action: WorkspaceAction::List,
        }
    }
}

/// The `medulla workflow` action.
///
/// The verbs split into four groups: authoring (`create`, `apply-ops`,
/// `validate`, `catalog`), inspection (`list`, `get`, `list-runs`, `get-run`),
/// execution (`dry-run`, `run`, `resume`, `cancel`), and review (`notes`,
/// `note`, `evolve`, `proposals`, `accept`, `reject`). A harness building a
/// workflow uses the first two; an operator uses all four.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowAction {
    /// `list` — every installed workflow.
    List,
    /// `get <id>` — one workflow, whole.
    Get(String),
    /// `create <id>` — install a workflow from a document on stdin.
    Create(String),
    /// `delete <id>` — uninstall a workflow.
    Delete(String),
    /// `apply-ops <id>` — apply graph patches read from stdin.
    ApplyOps(String),
    /// `preview-ops <id>` — check graph patches from stdin without saving.
    PreviewOps(String),
    /// `validate [id]` — validate a saved workflow, or a document on stdin.
    Validate(Option<String>),
    /// `dry-run <id>` — simulate, dispatching nothing.
    DryRun(String),
    /// `run <id>` — run for real against the coding CLIs on this machine.
    Run(String),
    /// `resume <run-id>` — release approval gates and continue.
    Resume(String),
    /// `cancel <run-id>` — stop a run executing in this process.
    Cancel(String),
    /// `list-runs <id>` — a workflow's run history.
    ListRuns(String),
    /// `get-run <run-id>` — one run record.
    GetRun(String),
    /// `catalog [kind]` — the node kinds an author may use.
    Catalog(Option<String>),
    /// `defaults <id>` — read, set, or clear the harness and model every step
    /// of a workflow runs on. With neither `--harness` nor `--model` it prints
    /// what is currently set rather than changing anything.
    Defaults(String),
    /// `notes <id>` — everything recorded about a workflow.
    Notes(String),
    /// `note <id>` — record a note; `--kind` and `--text` say what.
    AddNote(String),
    /// `evolve <id>` — review a workflow against its own history.
    Evolve(String),
    /// `author [id]` — run one copilot turn; `--text` carries the instruction.
    ///
    /// With an id it revises that workflow, without one it builds a new one.
    /// The same turn the Workflows pane runs, reachable without a terminal —
    /// which is what makes "does the harness actually get its tools?" a
    /// question a test can ask.
    Author(Option<String>),
    /// `proposals <id>` — changes proposed for a workflow.
    Proposals(String),
    /// `accept <proposal-id>` — apply a proposal to the saved graph.
    Accept(String),
    /// `reject <proposal-id>` — turn one down; `--reason` says why.
    Reject(String),
    /// `mcp` — serve the workflow tools over MCP on stdin/stdout.
    ///
    /// Not for a human to run: this is the command Medulla attaches to an ACP
    /// session so the harness on the other end can author workflows.
    Mcp,
}

/// Parsed `medulla workflow` flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowArgs {
    /// Explicit `--config` path. `None` selects the layered config discovery.
    pub config: Option<String>,
    /// `--input <json>`: the trigger payload for `run`/`dry-run`.
    pub input: Option<String>,
    /// `--inputs <json object>`: values for the workflow's declared inputs,
    /// keyed by name. Distinct from `--input`, which is the free-form trigger
    /// payload.
    pub inputs: Option<String>,
    /// `--set <name>=<value>`, repeatable: one declared input at a time, for
    /// the common case where hand-writing a JSON object at a shell prompt is
    /// more quoting than the value is worth. Merged over `--inputs`.
    pub set: Vec<String>,
    /// `--approve <node-id>`, repeatable: gates to release on `resume`.
    pub approve: Vec<String>,
    /// `--reject <node-id>`, repeatable: gates to refuse on `resume`.
    pub reject: Vec<String>,
    /// `--run-id <id>`: the id to give a new run, or the failed run an
    /// `evolve` should lead with. Absent mints one, or reviews the whole
    /// history.
    pub run_id: Option<String>,
    /// `--kind <kind>`: which kind of note `note` records.
    pub kind: Option<String>,
    /// `--text <text>`: the note itself, for `note`.
    pub text: Option<String>,
    /// `--reason <reason>`: why a proposal was rejected.
    pub reason: Option<String>,
    /// `--supersedes <note-id>`, repeatable: notes this one replaces.
    pub supersedes: Vec<String>,
    /// `--harness <name>`: the harness `defaults` pins for every step. An empty
    /// string clears it; absent leaves whatever is set alone.
    pub harness: Option<String>,
    /// `--model <name>`: the model hint `defaults` pins for every step. Empty
    /// clears, absent leaves alone.
    pub model: Option<String>,
    /// The selected action.
    pub action: WorkflowAction,
}

impl Default for WorkflowArgs {
    fn default() -> Self {
        WorkflowArgs {
            config: None,
            input: None,
            inputs: None,
            set: Vec::new(),
            approve: Vec::new(),
            reject: Vec::new(),
            run_id: None,
            kind: None,
            text: None,
            reason: None,
            supersedes: Vec::new(),
            harness: None,
            model: None,
            action: WorkflowAction::List,
        }
    }
}

/// Parsed `medulla login` flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoginArgs {
    /// Explicit `--config` path used only to resolve `backend.baseUrl`. `None`
    /// selects the layered config discovery.
    pub config: Option<String>,
    pub provider: Provider,
    pub no_browser: bool,
    /// A 64-hex one-time login token (headless fallback); skips the listener.
    pub token: Option<String>,
}

/// Parsed `medulla update` flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateArgs {
    /// `--check`: report whether an update is available and exit without
    /// installing anything.
    pub check: bool,
}

/// Parsed TUI flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiArgs {
    /// Explicit `--config` path. `None` selects the layered config discovery
    /// (`./.medulla/config.toml` / `./medulla.toml` / `<home>/config.toml`).
    pub config: Option<String>,
    pub alt_screen: bool,
    /// Force the offline demo runtime, skipping the token lookup and the login
    /// screen. The only headless way to reach a working runtime with no backend.
    pub mock: bool,
}

impl Default for TuiArgs {
    fn default() -> Self {
        TuiArgs {
            config: None,
            alt_screen: true,
            mock: false,
        }
    }
}

/// Parsed `medulla run` flags: the non-interactive one-instruction core-runtime
/// driver for scripting / e2e.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunArgs {
    /// Explicit `--config` path. `None` selects the layered config discovery.
    pub config: Option<String>,
    /// The instruction to submit — every non-flag argument, joined by spaces.
    pub instruction: String,
}
