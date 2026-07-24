//! Argument parsing for `main`: subcommand dispatch ([`parse_command`]) and the
//! per-subcommand flag parsers ([`parse_login_args`], [`parse_memory_args`],
//! [`parse_update_args`], [`parse_tui_args`]), plus the [`help_text`] shown by
//! `medulla help`/`--help`. Every function is pure over its input args.

use medulla::auth::Provider;
use medulla::tinyplace::HarnessProvider;

use super::types::{
    Command, InitArgs, LoginArgs, MemoryAction, MemoryArgs, RunArgs, TuiArgs, UpdateArgs,
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
        Some("memory") => Command::Memory,
        Some("update") => Command::Update,
        Some("init") => Command::Init,
        Some("hub") => Command::Hub,
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

/// Parse `medulla memory <action> [flags]`. Returns a usage error on a missing
/// or unknown action, or a `search` with no query.
pub fn parse_memory_args(args: &[String]) -> Result<MemoryArgs, String> {
    let action_word = args.first().map(String::as_str).ok_or_else(|| {
        "expected a subcommand: status|ingest|backfill|compile|search".to_string()
    })?;

    let mut config: Option<String> = None;
    let mut json = false;
    let mut facet: Option<String> = None;
    let mut k: usize = 5;
    let mut query_parts: Vec<String> = Vec::new();

    let mut it = args.iter().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => config = it.next().cloned(),
            "--json" => json = true,
            "--facet" => facet = it.next().cloned(),
            "--k" => {
                if let Some(v) = it.next() {
                    k = v
                        .parse::<usize>()
                        .map_err(|_| format!("invalid --k value '{v}'"))?;
                }
            }
            other => query_parts.push(other.to_string()),
        }
    }

    let action = match action_word {
        "status" => MemoryAction::Status,
        "ingest" => MemoryAction::Ingest,
        "backfill" => MemoryAction::Backfill,
        "compile" => MemoryAction::Compile,
        "search" => {
            let query = query_parts.join(" ");
            if query.trim().is_empty() {
                return Err("memory search: expected a query".to_string());
            }
            MemoryAction::Search(query)
        }
        other => return Err(format!("unknown memory subcommand '{other}'")),
    };

    Ok(MemoryArgs {
        config,
        json,
        facet,
        k,
        action,
    })
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
            "--core-socket" => {
                if let Some(v) = it.next() {
                    out.core_socket = Some(v.clone());
                }
            }
            "--no-alt-screen" => out.alt_screen = false,
            "--mock" => out.mock = true,
            _ => {}
        }
    }
    out
}

/// Parse `medulla run [flags] <instruction...>`. `--config` / `--core-socket`
/// take a value; every other non-flag token is part of the instruction, joined
/// by spaces. Returns a usage error when no instruction text is supplied.
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
            "--core-socket" => {
                if let Some(v) = it.next() {
                    out.core_socket = Some(v.clone());
                }
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
medulla daemon [flags]  Run the headless coding-agent daemon (serves tasks over tiny.place)\n  \
medulla daemon --tui    Run the daemon with its operator screen\n  \
                        --workspace <dir>      where peer tasks run\n  \
                        --no-trust-workspace   don't pre-trust it with claude\n  \
medulla sessions        List recent claude/codex sessions as JSON\n  \
medulla codex [args]    Run Codex in your terminal, bridged to tiny.place\n  \
medulla claude [args]   Run Claude Code in your terminal, bridged to tiny.place\n  \
medulla opencode [args] Run OpenCode in your terminal, bridged to tiny.place\n  \
medulla login [flags]   Log in to the backend and store credentials\n  \
medulla logout          Clear stored credentials\n  \
medulla memory <cmd>    Persona memory: status|ingest|backfill|compile|search <query>\n  \
medulla init [dir]      Write a MEDULLA.md workspace profile for a directory\n  \
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
--dangerously-skip-permissions  Pass the harness its skip-permissions flag\n\n\
Wrapper flags:\n  \
--no-bridge             Run the CLI as a plain passthrough (no tiny.place bridge)\n  \
--                      Pass all following arguments to the CLI verbatim\n\n\
Login flags:\n  \
--provider <name>       OAuth provider: google (default), github, twitter\n  \
--no-browser            Print the login URL without launching a browser\n  \
--token <64-hex>        Redeem a one-time login token instead (headless)\n  \
--config <path>         Config file to read backend.baseUrl from (.toml or .json)\n\n\
Memory flags:\n  \
--json                  Emit JSON instead of human-readable output\n  \
--facet <name>          Restrict a search to one facet\n  \
--k <n>                 Max search results (default 5)\n  \
--config <path>         Explicit config file (.toml or .json) for the memory section\n\n\
Init flags:\n  \
--force, -f             Overwrite an existing MEDULLA.md\n  \
--offline               Skip the model call and write an editable stub\n  \
--config <path>         Explicit config file (.toml or .json) for backend/model settings\n\n\
Run flags:\n  \
--core-socket <path>    medulla-serve unix socket to attach (else MEDULLA_CORE_SOCKET / [core] config)\n  \
--config <path>         Explicit config file (.toml or .json) for the [core] section\n\n\
TUI flags:\n  \
--config <path>         Explicit config file (.toml or .json); bypasses layered discovery\n  \
--core-socket <path>    Attach the core medulla-serve runtime at this socket instead of the backend\n  \
--mock                  Run the offline demo runtime (no backend, no login)\n  \
--no-alt-screen         Do not switch to the alternate screen\n",
        version = env!("CARGO_PKG_VERSION"),
    )
}
