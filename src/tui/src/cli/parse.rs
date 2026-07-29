//! Argument parsing for `main`: subcommand dispatch ([`parse_command`]) and the
//! per-subcommand flag parsers ([`parse_login_args`],
//! [`parse_update_args`], [`parse_tui_args`]), plus the [`help_text`] shown by
//! `medulla help`/`--help`. Every function is pure over its input args.

use medulla::auth::Provider;
use medulla::tinyplace::HarnessProvider;

use super::types::{
    Command, InitArgs, LoginArgs, RunArgs, TuiArgs, UpdateArgs, WorkflowAction, WorkflowArgs,
    WorkspaceAction, WorkspaceArgs,
};

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
    let mut it = args.iter();
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
            "--run-id" => {
                if let Some(v) = it.next() {
                    out.run_id = Some(v.clone());
                }
            }
            "--kind" => {
                if let Some(v) = it.next() {
                    out.kind = Some(v.clone());
                }
            }
            "--text" => {
                if let Some(v) = it.next() {
                    out.text = Some(v.clone());
                }
            }
            "--reason" => {
                if let Some(v) = it.next() {
                    out.reason = Some(v.clone());
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
medulla run <text>      Submit one instruction to a local medulla-serve socket and stream events (JSON lines)\n  \
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
medulla workflow mcp    Serve the workflow tools over MCP (for ACP sessions)\n  \
medulla update [--check] Update to the latest release (--check only reports)\n  \
medulla version         Print the version\n  \
medulla help            Show this help\n\n\
Daemon flags:\n  \
--tui                   Show the operator screen: sessions/log, contacts, requests\n  \
--providers <a,b>       Restrict to these coding agents (default: all found on PATH)\n  \
--workspace <dir>       Directory tasks run in (default: cwd)\n  \
--handle <name>         Register this tiny.place handle on startup\n  \
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
--run-id <id>           Id to give the run (default: a fresh one)\n  \
--approve <node-id>     Gate to release on resume (repeatable)\n  \
--reject <node-id>      Gate to refuse on resume (repeatable)\n\n\
Workflow documents and graph ops are read from stdin; every verb prints JSON.\n\n\
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
