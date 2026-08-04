//! Argument parsing for `main`: subcommand dispatch ([`parse_command`]) and the
//! per-subcommand flag parsers ([`parse_login_args`],
//! [`parse_update_args`], [`parse_tui_args`]), plus the [`help_text`] shown by
//! `medulla help`/`--help`. Every function is pure over its input args.

use medulla::auth::Provider;
use medulla::protocol::HarnessProvider;

use super::types::{
    Command, InitArgs, LoginArgs, RunArgs, TuiArgs, UpdateArgs, WorkflowAction, WorkflowArgs,
    WorkspaceAction, WorkspaceArgs,
};
#[cfg(feature = "workflows")]
use super::types::{SkillsAction, SkillsArgs};

/// Dispatch on the first argument. Anything else (including TUI flags) is the TUI.
pub fn parse_command(args: &[String]) -> Command {
    match args.first().map(String::as_str) {
        Some("run") => Command::Run,
        // `--tui` selects the worker-daemon screen instead of the headless
        // daemon. It is one process either way: the TUI *is* the daemon, so the
        // flag chooses a face, not a second program. Must precede the bare
        // `daemon` arm, which would otherwise match first and swallow the flag.
        Some("daemon") if args.iter().any(|a| a == "--tui") => Command::DaemonTui,
        Some("daemon") => Command::Daemon,
        Some("version") | Some("--version") | Some("-v") => Command::Version,
        Some("help") | Some("--help") | Some("-h") => Command::Help,
        Some("sessions") => Command::Sessions,
        Some("login") => Command::Login,
        Some("logout") => Command::Logout,
        Some("update") => Command::Update,
        Some("init") => Command::Init,
        Some("workspace") | Some("workspaces") => Command::Workspace,
        Some("hub") => Command::Hub,
        Some("workflow") | Some("workflows") => Command::Workflow,
        Some("skills") | Some("skill") => Command::Skills,
        Some("mcp") => Command::Mcp,
        Some("codex") => Command::Wrapper(HarnessProvider::Codex),
        Some("claude") => Command::Wrapper(HarnessProvider::Claude),
        Some("opencode") => Command::Wrapper(HarnessProvider::Opencode),
        _ => Command::Tui,
    }
}

/// Parse `medulla login` flags out of the args following `login`. Returns the
/// offending flag name on an unknown `--provider` value.
pub fn parse_login_args(args: &[String]) -> Result<LoginArgs, String> {
    let mut out = LoginArgs::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                if let Some(v) = it.next() {
                    out.config = Some(v.clone());
                }
            }
            "--provider" => {
                if let Some(v) = it.next() {
                    out.provider =
                        Provider::parse(v).ok_or_else(|| format!("unknown provider '{v}'"))?;
                }
            }
            "--token" => {
                if let Some(v) = it.next() {
                    out.token = Some(v.clone());
                }
            }
            "--no-browser" => out.no_browser = true,
            _ => {}
        }
    }
    Ok(out)
}

/// Parse the flags following `medulla update`.
pub fn parse_update_args(args: &[String]) -> UpdateArgs {
    let mut out = UpdateArgs::default();
    for arg in args {
        if arg == "--check" {
            out.check = true;
        }
    }
    out
}

/// Parse the flags following `medulla init`. The first non-flag argument is the
/// target directory; everything else defaults, so a bare `medulla init` targets
/// the current working directory.
pub fn parse_init_args(args: &[String]) -> InitArgs {
    let mut out = InitArgs::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                if let Some(v) = it.next() {
                    out.config = Some(v.clone());
                }
            }
            "--force" | "-f" => out.force = true,
            "--offline" => out.offline = true,
            other => {
                // First bare word is the directory; later ones are ignored.
                if !other.starts_with('-') && out.dir.is_none() {
                    out.dir = Some(other.to_string());
                }
            }
        }
    }
    out
}

/// Parse `medulla workspace` args out of the args following `workspace`.
///
/// The first bare word selects the action; a second bare word is its operand
/// (the directory for `add`, the directory or registry id for `remove`). An
/// unrecognised or absent action lists the registry, which is the harmless
/// read-only answer to "I typed something roughly like this".
///
/// `remove` with no operand also falls back to listing rather than guessing at a
/// target — deleting the wrong registry entry on a typo is not recoverable from
/// the output.
pub fn parse_workspace_args(args: &[String]) -> WorkspaceArgs {
    let mut out = WorkspaceArgs::default();
    let mut bare: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                if let Some(v) = it.next() {
                    out.config = Some(v.clone());
                }
            }
            "--harness" => {
                if let Some(v) = it.next() {
                    out.harness = Some(v.clone());
                }
            }
            "--force" | "-f" => out.force = true,
            "--offline" => out.offline = true,
            "--json" => out.json = true,
            other => {
                if !other.starts_with('-') {
                    bare.push(other.to_string());
                }
            }
        }
    }

    let operand = bare.get(1).cloned();
    out.action = match bare.first().map(String::as_str) {
        Some("add") | Some("init") => WorkspaceAction::Add(operand),
        Some("remove") | Some("rm") => match operand {
            Some(target) => WorkspaceAction::Remove(target),
            None => WorkspaceAction::List,
        },
        _ => WorkspaceAction::List,
    };
    out
}

/// Parse `medulla workflow` flags out of the args following `workflow`.
///
/// Unknown flags and stray operands are ignored rather than rejected, matching
/// the other subcommand parsers: this is called from a shell and from a harness
/// alike, and a hard failure on an extra token helps neither.
pub fn parse_workflow_args(args: &[String]) -> WorkflowArgs {
    let mut out = WorkflowArgs::default();
    let mut bare: Vec<String> = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                if let Some(v) = it.next() {
                    out.config = Some(v.clone());
                }
            }
            "--input" => {
                if let Some(v) = it.next() {
                    out.input = Some(v.clone());
                }
            }
            "--inputs" => {
                if let Some(v) = it.next() {
                    out.inputs = Some(v.clone());
                }
            }
            "--set" => {
                if let Some(v) = it.next() {
                    out.set.push(v.clone());
                }
            }
            "--run-id" => {
                if let Some(v) = it.next() {
                    out.run_id = Some(v.clone());
                }
            }
            "--kind" => {
                if it.peek().is_some_and(|value| !value.starts_with('-')) {
                    out.kind = it.next().cloned();
                }
            }
            "--text" => {
                if it.peek().is_some_and(|value| !value.starts_with('-')) {
                    out.text = it.next().cloned();
                }
            }
            // Empty is meaningful for both — it is how an operator clears a
            // pinned harness or model — so these accept a value that a
            // `!starts_with('-')` guard would otherwise have to allow anyway.
            "--harness" => {
                if it.peek().is_some_and(|value| !value.starts_with('-')) {
                    out.harness = it.next().cloned();
                }
            }
            "--model" => {
                if it.peek().is_some_and(|value| !value.starts_with('-')) {
                    out.model = it.next().cloned();
                }
            }
            "--reason" => {
                if it.peek().is_some_and(|value| !value.starts_with('-')) {
                    out.reason = it.next().cloned();
                }
            }
            // Repeatable: one conclusion can replace several earlier guesses.
            "--supersedes" => {
                if it.peek().is_some_and(|value| !value.starts_with('-')) {
                    out.supersedes
                        .push(it.next().expect("peeked value exists").clone());
                }
            }
            // Repeatable: a paused run may be holding more than one gate, and
            // releasing them one call at a time would restart the run each time.
            "--approve" => {
                if let Some(v) = it.next() {
                    out.approve.push(v.clone());
                }
            }
            "--reject" => {
                if let Some(v) = it.next() {
                    out.reject.push(v.clone());
                }
            }
            other => {
                if !other.starts_with('-') {
                    bare.push(other.to_string());
                }
            }
        }
    }

    let operand = bare.get(1).cloned();
    out.action = match bare.first().map(String::as_str) {
        Some("get") | Some("show") => operand.map_or(WorkflowAction::List, WorkflowAction::Get),
        Some("create") | Some("save") => {
            operand.map_or(WorkflowAction::List, WorkflowAction::Create)
        }
        Some("delete") | Some("rm") => operand.map_or(WorkflowAction::List, WorkflowAction::Delete),
        Some("apply-ops") | Some("edit") => {
            operand.map_or(WorkflowAction::List, WorkflowAction::ApplyOps)
        }
        Some("preview-ops") => operand.map_or(WorkflowAction::List, WorkflowAction::PreviewOps),
        // The one verb whose operand is genuinely optional: with an id it
        // checks a saved workflow, without one it reads a document from stdin.
        Some("validate") | Some("check") => WorkflowAction::Validate(operand),
        Some("dry-run") | Some("simulate") => {
            operand.map_or(WorkflowAction::List, WorkflowAction::DryRun)
        }
        Some("run") => operand.map_or(WorkflowAction::List, WorkflowAction::Run),
        Some("resume") => operand.map_or(WorkflowAction::List, WorkflowAction::Resume),
        Some("cancel") => operand.map_or(WorkflowAction::List, WorkflowAction::Cancel),
        Some("list-runs") | Some("runs") => {
            operand.map_or(WorkflowAction::List, WorkflowAction::ListRuns)
        }
        Some("get-run") => operand.map_or(WorkflowAction::List, WorkflowAction::GetRun),
        Some("catalog") | Some("kinds") => WorkflowAction::Catalog(operand),
        Some("defaults") => operand.map_or(WorkflowAction::List, WorkflowAction::Defaults),
        Some("notes") => operand.map_or(WorkflowAction::List, WorkflowAction::Notes),
        Some("note") => operand.map_or(WorkflowAction::List, WorkflowAction::AddNote),
        Some("evolve") | Some("review") => {
            operand.map_or(WorkflowAction::List, WorkflowAction::Evolve)
        }
        Some("proposals") => operand.map_or(WorkflowAction::List, WorkflowAction::Proposals),
        Some("accept") => operand.map_or(WorkflowAction::List, WorkflowAction::Accept),
        Some("reject") => operand.map_or(WorkflowAction::List, WorkflowAction::Reject),
        Some("mcp") => WorkflowAction::Mcp,
        _ => WorkflowAction::List,
    };
    out
}

/// The `medulla skills` flags that take a value, as `--flag value`.
#[cfg(feature = "workflows")]
const SKILLS_VALUE_FLAGS: [&str; 4] = ["--harness", "--scope", "--dir", "--tools"];

/// The `medulla skills` flags that take no value.
#[cfg(feature = "workflows")]
const SKILLS_BOOL_FLAGS: [&str; 6] = [
    "--with-mcp",
    "--with-commands",
    "--dry-run",
    "--prune",
    "--all",
    "--json",
];

/// Rewrite `--flag=value` into `--flag`, `value` so the main loop sees one
/// spelling.
///
/// Only the flag token is split, and only on its *first* `=`, so a value that
/// itself contains one (`--dir /srv/a=b`) is untouched.
///
/// # Errors
///
/// Returns an operator-facing sentence when a value flag is given no value
/// (`--dir=`), when a boolean flag is given one (`--json=true`), or when the
/// head of an `=` form is not a flag this verb has. Each of those is a request
/// the operator meant, and honouring the rest of the command line while
/// discarding it is how a scratch-directory run becomes a `$HOME` run.
#[cfg(feature = "workflows")]
fn split_skills_flag_values(args: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        let Some((head, value)) = arg.split_once('=') else {
            out.push(arg.clone());
            continue;
        };
        if !head.starts_with('-') {
            out.push(arg.clone());
            continue;
        }
        if SKILLS_VALUE_FLAGS.contains(&head) {
            if value.is_empty() {
                return Err(format!("{head} was given no value"));
            }
            out.push(head.to_string());
            out.push(value.to_string());
        } else if SKILLS_BOOL_FLAGS.contains(&head) {
            return Err(format!("{head} takes no value (got `{arg}`)"));
        } else {
            return Err(format!(
                "unknown flag `{head}` for `medulla skills`. Run `medulla help` for the flags \
                 this verb takes."
            ));
        }
    }
    Ok(out)
}

/// Parse `medulla skills` flags out of the args following `skills`.
///
/// Stricter than its sibling parsers, and deliberately so: every flag here
/// decides *where files are written*, so a flag that does not arrive is not a
/// harmless no-op. A `--dir=/tmp/x` that was quietly dropped retargets the
/// whole operation at `$HOME` while still reporting success — and with
/// `uninstall --all` that is the difference between clearing a scratch
/// directory and clearing the operator's real one. So both spellings of a value
/// flag are accepted, and an unrecognised flag is an error rather than a bare
/// word, which is the opposite of what the rest of the CLI does.
///
/// The first bare word selects the verb; every later bare word is a workflow id.
///
/// # Errors
///
/// Returns an operator-facing sentence when `--harness`, `--scope`, or
/// `--tools` is given a value that is not one of its accepted ones, when one of
/// them is given no value at all, when a flag is not recognised or is given a
/// value it does not take, or when `--prune` is asked for alongside explicit
/// workflow ids — a combination whose two possible meanings differ by which
/// files get deleted.
#[cfg(feature = "workflows")]
pub fn parse_skills_args(args: &[String]) -> Result<SkillsArgs, String> {
    use medulla::workflows::skills::{SkillScope, SkillTarget};

    let args = split_skills_flag_values(args)?;
    let args = args.as_slice();
    let mut out = SkillsArgs::default();
    let mut bare: Vec<String> = Vec::new();
    // Accumulated separately from `out.targets` so that "no --harness at all"
    // stays distinguishable from "--harness with an empty list", which the
    // runner treats very differently.
    let mut targets: Option<Vec<SkillTarget>> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            // Repeatable *and* comma-joined: `--harness claude --harness codex`
            // and `--harness claude,codex` are the same request, because both
            // spellings turn up in shell history and neither is wrong.
            "--harness" => {
                let value = it.next().ok_or_else(|| {
                    "--harness expects claude, codex, generic, or all".to_string()
                })?;
                let chosen = targets.get_or_insert_with(Vec::new);
                if value.trim().eq_ignore_ascii_case("all") {
                    chosen.extend(SkillTarget::ALL);
                } else {
                    for part in value.split(',').filter(|part| !part.trim().is_empty()) {
                        chosen.push(SkillTarget::parse(part)?);
                    }
                }
            }
            "--scope" => {
                let value = it
                    .next()
                    .ok_or_else(|| "--scope expects user or project".to_string())?;
                out.scope = SkillScope::parse(value)?;
            }
            "--dir" => {
                let value = it
                    .next()
                    .ok_or_else(|| "--dir expects a directory path".to_string())?;
                out.dir = Some(value.clone());
            }
            "--tools" => {
                let value = it
                    .next()
                    .ok_or_else(|| "--tools expects run or full".to_string())?;
                out.tools = match value.trim().to_ascii_lowercase().as_str() {
                    "run" => "run".to_string(),
                    "full" => "full".to_string(),
                    other => {
                        return Err(format!(
                            "unknown tool mode `{other}` (expected run or full)"
                        ))
                    }
                };
            }
            "--with-mcp" => out.with_mcp = true,
            "--with-commands" => out.with_commands = true,
            "--dry-run" => out.dry_run = true,
            "--prune" => out.prune = true,
            "--all" => out.all = true,
            "--json" => out.json = true,
            other if other.starts_with('-') => {
                return Err(format!(
                    "unknown flag `{other}` for `medulla skills`. Run `medulla help` for the \
                     flags this verb takes."
                ));
            }
            other => bare.push(other.to_string()),
        }
    }

    // Duplicates are a natural consequence of accepting both spellings; writing
    // the same file twice would double every line of the report.
    if let Some(chosen) = targets.as_mut() {
        let mut seen = Vec::new();
        chosen.retain(|target| {
            let fresh = !seen.contains(target);
            seen.push(*target);
            fresh
        });
    }
    out.targets = targets;

    out.action = match bare.first().map(String::as_str) {
        Some("install") | Some("add") => SkillsAction::Install,
        Some("sync") | Some("refresh") => SkillsAction::Sync,
        Some("uninstall") | Some("remove") | Some("rm") => SkillsAction::Uninstall,
        _ => SkillsAction::List,
    };
    // Only ids, never the verb: `skills install` must not try to install a
    // workflow called "install".
    out.ids = bare.into_iter().skip(1).collect();

    // `sync <ids> --prune` has no honest reading. Pruning asks "which managed
    // skills belong to no live workflow?", which is a question about the whole
    // store; answering it from a one-id keep set would delete the skills for
    // every workflow the operator did not happen to name. Refusing beats
    // guessing at which of the two surprises they wanted.
    if out.action == SkillsAction::Sync && out.prune && !out.ids.is_empty() {
        return Err(format!(
            "--prune cannot be combined with workflow ids ({}): pruning decides what to remove by \
             looking at every workflow, so restricting it to the ids you named would delete the \
             skills for the ones you did not. Run `medulla skills sync --prune` for all of them, \
             or `medulla skills sync {}` without --prune.",
            out.ids.join(", "),
            out.ids.join(" "),
        ));
    }
    Ok(out)
}

/// Parse the TUI's own flags out of `argv[1..]`.
pub fn parse_tui_args(args: &[String]) -> TuiArgs {
    let mut out = TuiArgs::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                if let Some(v) = it.next() {
                    out.config = Some(v.clone());
                }
            }
            "--no-alt-screen" => out.alt_screen = false,
            "--mock" => out.mock = true,
            _ => {}
        }
    }
    out
}

/// Parse `medulla run [flags] <instruction...>`. `--config` takes a value;
/// every other non-flag token is part of the instruction, joined by spaces.
/// Returns a usage error when no instruction text is supplied, or when the
/// retired `--core-socket` flag is passed.
pub fn parse_run_args(args: &[String]) -> Result<RunArgs, String> {
    let mut out = RunArgs::default();
    let mut instruction: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                if let Some(v) = it.next() {
                    out.config = Some(v.clone());
                }
            }
            // Rejected rather than ignored: an unrecognized token here joins
            // the instruction, so quietly dropping this arm would submit
            // "--core-socket /path ..." to the agent as prompt text.
            "--core-socket" => {
                return Err(
                    "run: --core-socket is gone; the core is embedded in this process".to_string(),
                )
            }
            other => instruction.push(other.to_string()),
        }
    }
    out.instruction = instruction.join(" ");
    if out.instruction.trim().is_empty() {
        return Err("run: expected an instruction to submit".to_string());
    }
    Ok(out)
}

/// The `medulla help` / `--help` text.
pub fn help_text() -> String {
    format!(
        "medulla {version}\n\n\
Usage:\n  \
medulla                 Start the interactive chat TUI (default)\n  \
medulla run <text>      Submit one instruction to the embedded core and stream the cycle's events (JSON lines)\n  \
medulla daemon [flags]  Run the daemon TUI (agents, master, workspaces, requests)\n  \
medulla daemon --headless  Run without the operator screen (automatic when piped)\n  \
                        --workspace <dir>      where peer tasks run\n  \
                        --no-trust-workspace   don't pre-trust it with claude\n  \
medulla sessions        List recent claude/codex sessions as JSON\n  \
medulla codex [args]    Run Codex in your terminal, bridged to tiny.place\n  \
medulla claude [args]   Run Claude Code in your terminal, bridged to tiny.place\n  \
medulla opencode [args] Run OpenCode in your terminal, bridged to tiny.place\n  \
medulla login [flags]   Log in to the backend and store credentials\n  \
medulla logout          Clear stored credentials\n  \
medulla init [dir]      Write a MEDULLA.md workspace profile for a directory\n  \
medulla workspace <cmd> Workspace registry: add [dir]|list|remove <dir|id>\n  \
medulla workflow <cmd>  Workflows: list|get|create|apply-ops|validate|dry-run|run|resume|cancel|catalog\n  \
medulla skills <cmd>    Harness skills for workflows: list|install|sync|uninstall\n  \
medulla mcp             Serve Medulla's tools over MCP (spawned for harnesses)\n  \
medulla workflow mcp    Alias for `medulla mcp`, workflow tools only\n  \
medulla update [--check] Update to the latest release (--check only reports)\n  \
medulla version         Print the version\n  \
medulla help            Show this help\n\n\
Daemon flags:\n  \
--tui                   Show the operator screen: sessions/log, contacts, requests\n  \
--providers <a,b>       Restrict to these coding agents (default: all found on PATH)\n  \
--workspace <dir>       Directory tasks run in (default: cwd)\n  \
--model <name>          Default model hint passed to the harness\n  \
--concurrency <n>       Maximum tasks running at once\n  \
--once                  Drain the inbox once and exit (probe)\n  \
--no-onboard            Skip key publishing and directory registration\n  \
--no-pair               Do not print the pairing block or copy the address\n  \
--dangerously-skip-permissions  Pass the harness its skip-permissions flag\n\n\
Wrapper flags:\n  \
--no-bridge             Run the CLI as a plain passthrough (no tiny.place bridge)\n  \
--                      Pass all following arguments to the CLI verbatim\n\n\
Workflow flags:\n  \
--input <json>          Trigger payload for run / dry-run\n  \
--inputs <json>         Declared workflow inputs, as an object keyed by name\n  \
--set <name>=<value>    One declared input (repeatable; wins over --inputs)\n  \
--run-id <id>           Id to give the run (default: a fresh one)\n  \
--approve <node-id>     Gate to release on resume (repeatable)\n  \
--reject <node-id>      Gate to refuse on resume (repeatable)\n\n\
Workflow documents and graph ops are read from stdin; every verb prints JSON.\n\n\
Skills flags:\n  \
--harness <a,b>         claude, codex, generic, or all (default: those already set up)\n  \
--scope <user|project>  Install into $HOME or into this checkout (default: user)\n  \
--dir <path>            Explicit root, overriding --scope\n  \
--with-mcp              Also register `medulla mcp` with each harness\n  \
--with-commands         Also write the slash-command variant\n  \
--tools <run|full>      Tool surface a skill-triggered session gets (default: run)\n  \
--prune                 Remove skills for workflows that are gone (sync, no ids)\n  \
--all                   Remove every managed skill (uninstall, instead of ids)\n  \
--dry-run               Report what would change and write nothing\n  \
--json                  Emit JSON instead of the human summary\n\n\
Login flags:\n  \
--provider <name>       OAuth provider: google (default), github, twitter\n  \
--no-browser            Print the login URL without launching a browser\n  \
--token <64-hex>        Redeem a one-time login token instead (headless)\n  \
--config <path>         Config file to read backend.baseUrl from (.toml or .json)\n\n\
Init flags:\n  \
--force, -f             Overwrite an existing MEDULLA.md\n  \
--offline               Skip the model call and write an editable stub\n  \
--config <path>         Explicit config file (.toml or .json) for backend/model settings\n\n\
Workspace flags:\n  \
--harness <id>          Attach the added workspace to this harness (add)\n  \
--force, -f             Overwrite an existing MEDULLA.md (add)\n  \
--offline               Skip the model call and write an editable stub (add)\n  \
--json                  Emit JSON instead of human-readable output (list)\n  \
--config <path>         Explicit config file (.toml or .json) holding the registry\n\n\
Run flags:\n  \
--config <path>         Explicit config file (.toml or .json)\n\n\
TUI flags:\n  \
--config <path>         Explicit config file (.toml or .json); bypasses layered discovery\n  \
--mock                  Run the offline demo runtime (no backend, no login)\n  \
--no-alt-screen         Do not switch to the alternate screen\n",
        version = env!("CARGO_PKG_VERSION"),
    )
}
