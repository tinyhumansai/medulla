//! Registering the Medulla MCP server with the harness that will read the
//! generated skills.
//!
//! A skill is only instructions; the tool call it tells the model to make
//! (`workflow_run`) exists solely because `medulla mcp` is attached to that
//! session. Installing skills without registering the server produces a harness
//! that confidently reaches for a tool it does not have, so
//! [`install`](super::install) pairs with this module.
//!
//! Two properties are deliberate:
//!
//! * **Nothing here shells out.** Registration is a config-file merge, and a
//!   merge we perform ourselves is inspectable, testable offline, and identical
//!   under `--dry-run`. Spawning `claude mcp add` would make the result depend
//!   on which CLI version happens to be on `PATH`.
//! * **Nothing here is clobbered.** Every writable target is a read-merge-write
//!   over the operator's existing file: other servers, other top-level keys, and
//!   unrelated tables all survive. `~/.codex/config.toml` is hand-written, so it
//!   is edited with `toml_edit` and keeps its comments, blank lines, and key
//!   order too — a value-level round-trip through `toml::Table` would have
//!   deleted all three on the first registration. When the desired entry is
//!   already present in meaning, the file is not rewritten at all, which is what
//!   makes re-running the command a no-op instead of a formatting churn.
//!
//! ## Why Claude user scope writes nothing
//!
//! Claude Code reads project MCP servers from a checked-in `.mcp.json`, which is
//! a stable, documented file we can merge into. Its *user*-scope registry has no
//! such contract — it lives inside the CLI's own state, whose location and shape
//! are an implementation detail that has already moved. Writing a file we guessed
//! at would produce a registration Claude never reads while reporting success, so
//! user scope instead returns the exact `claude mcp add` line for the operator to
//! run. `Generic` is the same bargain for a harness we have not verified at all.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, TableLike, Value as TomlValue};

use super::types::SkillScope;
use super::SkillTarget;
use crate::workflows::mcp::{ToolMode, TOOL_MODE_ENV};

/// The server key written into every harness config.
const SERVER_NAME: &str = "medulla";

/// The single argument the registered command takes: `medulla mcp`.
const MCP_ARG: &str = "mcp";

/// The environment variable that selects which workflow tools `medulla mcp`
/// exposes; carried into the registration so a skill-triggered session gets the
/// same tool surface the operator chose at install time.
///
/// The same variable [`ToolMode`] reads, named once so a registration cannot
/// come to disagree with the server it configures.
const TOOLS_ENV: &str = TOOL_MODE_ENV;

/// Every [`ToolMode`], so a supplied string can be *recognised* rather than
/// merely resolved.
///
/// Enumerated here because [`ToolMode::from_wire`] answers every string: an
/// unknown one falls closed to [`ToolMode::Propose`] instead of failing, which
/// is right for a spawned server reading its environment and wrong for a
/// registration we are about to write to disk. `assert_tool_modes_enumerated`
/// is what stops this list from drifting when a variant is added.
const TOOL_MODES: [ToolMode; 3] = [ToolMode::Full, ToolMode::Propose, ToolMode::Run];

/// Compile-time guard for [`TOOL_MODES`]: a new [`ToolMode`] variant makes this
/// match non-exhaustive, and whoever adds it has to extend the list above.
#[allow(dead_code)]
fn assert_tool_modes_enumerated(mode: ToolMode) -> usize {
    match mode {
        ToolMode::Full => 0,
        ToolMode::Propose => 1,
        ToolMode::Run => 2,
    }
}

/// Where and how to register the MCP server.
///
/// `root` and `project_dir` are separate because the two writable targets
/// disagree about what they mean: Codex's config always lives under the scope
/// root (`~/.codex` or `<project>/.codex`), while Claude's `.mcp.json` is a
/// property of the checkout regardless of scope. Both are supplied by the caller
/// already resolved, so a test can point them at tempdirs.
#[derive(Debug, Clone)]
pub struct RegistrationOptions {
    /// Which harnesses to register with, in report order.
    pub targets: Vec<SkillTarget>,
    /// The scope the caller installed skills under. Only Claude reads it, to
    /// decide between merging `.mcp.json` and returning a manual command.
    pub scope: SkillScope,
    /// The resolved scope root — `$HOME` or the project directory.
    pub root: PathBuf,
    /// The project checkout that owns `.mcp.json`.
    pub project_dir: PathBuf,
    /// Absolute path of the `medulla` binary to launch. Absolute on purpose: a
    /// registration is read by a process whose `PATH` we do not control.
    pub exe: PathBuf,
    /// Value for [`TOOLS_ENV`]: a [`ToolMode`] wire name that can start a
    /// workflow, so `run` or `full`.
    ///
    /// Validated by [`register`], which refuses anything else rather than write
    /// a registration whose sessions could never trigger the skills it pairs
    /// with — `ToolMode::from_wire` answers an unrecognised value with
    /// [`ToolMode::Propose`], and a propose session is served no `workflow_run`.
    pub tools_mode: String,
    /// Compute identical outcomes but write nothing.
    pub dry_run: bool,
}

/// What registration did — or would have done — for one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationOutcome {
    /// The harness this line is about.
    pub target: SkillTarget,
    /// The config file touched, or `None` when the target has no file to write.
    pub path: Option<PathBuf>,
    /// One of `created`, `updated`, `unchanged`, `manual`, or `skipped`.
    ///
    /// `manual` means the target has no file we can safely write and
    /// [`manual_command`](Self::manual_command) carries the line to run instead.
    /// `skipped` means an existing config could not be parsed and was left
    /// untouched, with the reason in `manual_command`.
    pub action: String,
    /// A command for the operator to run by hand, set for `manual` and
    /// `skipped`.
    pub manual_command: Option<String>,
}

/// Registers `medulla mcp` with every configured target.
///
/// Merges into each writable target's config, preserving all other servers and
/// keys, and returns one outcome per target in `opts.targets` order. Targets
/// that cannot be written safely report `manual` with the command to run.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`], before touching any file, when
/// `opts.tools_mode` is not a tool mode that can start a workflow. Otherwise
/// propagates filesystem errors from creating parent directories, reading, or
/// writing a config. A config that exists but does not parse is *not* an error:
/// it is reported as `skipped` so one broken file does not abort the rest.
pub fn register(opts: &RegistrationOptions) -> io::Result<Vec<RegistrationOutcome>> {
    check_tools_mode(&opts.tools_mode)?;
    let mut outcomes = Vec::with_capacity(opts.targets.len());
    for target in &opts.targets {
        outcomes.push(match target {
            // Nothing to register in the managed root, whatever the harness: a
            // session Medulla spawned is handed the MCP server at session/new
            // with a grant minted for it, so a config file there would be a
            // second, weaker copy of something already true. Reported rather
            // than skipped silently, so `--with-mcp` never looks like it did
            // something it did not.
            _ if opts.scope == SkillScope::Managed => {
                already_attached(*target, opts.tools_mode.as_str())
            }
            SkillTarget::Claude => match opts.scope {
                SkillScope::Project => register_claude_project(opts)?,
                SkillScope::Managed | SkillScope::User => {
                    manual(SkillTarget::Claude, claude_user_command(opts))
                }
            },
            SkillTarget::Codex => register_codex(opts)?,
            SkillTarget::Generic => manual(SkillTarget::Generic, generic_command(opts)),
        });
    }
    Ok(outcomes)
}

/// Rejects a tool mode that would produce a registration nothing can trigger.
///
/// Two failures, both silent without this check. An unrecognised spelling is
/// resolved by [`ToolMode::from_wire`] to [`ToolMode::Propose`] rather than
/// rejected, and `Propose` — however it was arrived at — is served no
/// `workflow_run`. Either way the operator gets a server their skills tell the
/// model to call and a session that cannot honour the call.
///
/// [`ToolMode::as_wire`] and [`ToolMode::allows`] are the only authorities used,
/// so the accepted set follows the server's own tool surface.
fn check_tools_mode(mode: &str) -> io::Result<()> {
    let Some(recognised) = TOOL_MODES.iter().find(|known| known.as_wire() == mode) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "`{mode}` is not a {TOOLS_ENV} value (expected one of: {})",
                runnable_modes().join(", ")
            ),
        ));
    };
    if !recognised.allows("workflow_run") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "`{mode}` withholds workflow_run, so a session registered with it could never \
                 start a workflow (expected one of: {})",
                runnable_modes().join(", ")
            ),
        ));
    }
    Ok(())
}

/// The wire names of the tool modes that can start a workflow.
fn runnable_modes() -> Vec<&'static str> {
    TOOL_MODES
        .iter()
        .filter(|mode| mode.allows("workflow_run"))
        .map(|mode| mode.as_wire())
        .collect()
}

/// The `claude mcp add` invocation that registers the same server at user scope.
fn claude_user_command(opts: &RegistrationOptions) -> String {
    format!(
        "claude mcp add --scope user {SERVER_NAME} --env {TOOLS_ENV}={} -- {} mcp",
        opts.tools_mode,
        opts.exe.display()
    )
}

/// The instruction for a harness whose config format we do not know: whatever
/// its MCP client configuration is, this is the process it should launch.
fn generic_command(opts: &RegistrationOptions) -> String {
    format!(
        "configure your MCP client to run: {TOOLS_ENV}={} {} mcp",
        opts.tools_mode,
        opts.exe.display()
    )
}

/// A no-file outcome carrying the command the operator must run.
fn manual(target: SkillTarget, command: String) -> RegistrationOutcome {
    RegistrationOutcome {
        target,
        path: None,
        action: "manual".to_string(),
        manual_command: Some(command),
    }
}

/// The outcome for the managed root: the server is already attached, by
/// Medulla, to every session it starts — with the per-session grant that
/// decides the tool families, which no config file can express.
fn already_attached(target: SkillTarget, tools_mode: &str) -> RegistrationOutcome {
    RegistrationOutcome {
        target,
        path: None,
        action: "already-attached".to_string(),
        manual_command: Some(format!(
            "sessions Medulla spawns are served the MCP tools directly (mode `{tools_mode}` is \
             decided per session), so the managed root needs no registration"
        )),
    }
}

/// A left-alone outcome for a config we could not parse.
fn skipped(target: SkillTarget, path: &Path, reason: String) -> RegistrationOutcome {
    RegistrationOutcome {
        target,
        path: Some(path.to_path_buf()),
        action: "skipped".to_string(),
        manual_command: Some(reason),
    }
}

/// Merges `mcpServers.medulla` into `<project_dir>/.mcp.json`.
fn register_claude_project(opts: &RegistrationOptions) -> io::Result<RegistrationOutcome> {
    let path = opts.project_dir.join(".mcp.json");
    let existing = match read_if_present(&path)? {
        Existing::Absent => None,
        Existing::Unreadable => {
            return Ok(skipped(
                SkillTarget::Claude,
                &path,
                format!(
                    "{} is not valid UTF-8; left untouched — add the medulla server by hand or \
                     move the file aside",
                    path.display()
                ),
            ));
        }
        Existing::Text(text) => Some(text),
    };

    let mut document = match existing.as_deref() {
        None => Map::new(),
        Some(text) if text.trim().is_empty() => Map::new(),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => map,
            Ok(_) | Err(_) => {
                return Ok(skipped(
                    SkillTarget::Claude,
                    &path,
                    format!(
                        "{} is not a JSON object; add the medulla server by hand or move the file aside",
                        path.display()
                    ),
                ));
            }
        },
    };

    let desired = claude_entry(opts);
    let servers = match document
        .entry("mcpServers".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
    {
        Value::Object(map) => map,
        _ => {
            return Ok(skipped(
                SkillTarget::Claude,
                &path,
                "`mcpServers` exists but is not an object; fix it and re-run".to_string(),
            ));
        }
    };

    if servers.get(SERVER_NAME) == Some(&desired) {
        return Ok(outcome(SkillTarget::Claude, path, "unchanged"));
    }
    let action = if existing.is_some() {
        "updated"
    } else {
        "created"
    };
    servers.insert(SERVER_NAME.to_string(), desired);

    if !opts.dry_run {
        let mut body = serde_json::to_string_pretty(&Value::Object(document))?;
        body.push('\n');
        write_new_file(&path, &body)?;
    }
    Ok(outcome(SkillTarget::Claude, path, action))
}

/// The `.mcp.json` value for our server.
fn claude_entry(opts: &RegistrationOptions) -> Value {
    let mut env = Map::new();
    env.insert(
        TOOLS_ENV.to_string(),
        Value::String(opts.tools_mode.clone()),
    );

    let mut entry = Map::new();
    entry.insert(
        "command".to_string(),
        Value::String(opts.exe.display().to_string()),
    );
    entry.insert(
        "args".to_string(),
        Value::Array(vec![Value::String(MCP_ARG.to_string())]),
    );
    entry.insert("env".to_string(), Value::Object(env));
    Value::Object(entry)
}

/// Merges `[mcp_servers.medulla]` into `<root>/.codex/config.toml`.
///
/// Edited with `toml_edit`, not `toml`: `toml::Table` models values only, so
/// reading a config into one and writing it back deletes every comment and
/// re-sorts every key. `~/.codex/config.toml` is a file the operator wrote by
/// hand, and the very first registration would have eaten it. `toml_edit` keeps
/// the document's trivia, so the write is an *edit* — our table appended or its
/// changed keys reassigned, and nothing else in the file moved.
fn register_codex(opts: &RegistrationOptions) -> io::Result<RegistrationOutcome> {
    let path = opts.root.join(".codex").join("config.toml");
    let existing = match read_if_present(&path)? {
        Existing::Absent => None,
        Existing::Unreadable => {
            return Ok(skipped(
                SkillTarget::Codex,
                &path,
                format!(
                    "{} is not valid UTF-8; left untouched — add [mcp_servers.medulla] by hand \
                     or move the file aside",
                    path.display()
                ),
            ));
        }
        Existing::Text(text) => Some(text),
    };

    let mut document = match existing.as_deref() {
        None => DocumentMut::new(),
        Some(text) => match text.parse::<DocumentMut>() {
            Ok(document) => document,
            Err(error) => {
                return Ok(skipped(
                    SkillTarget::Codex,
                    &path,
                    format!("could not parse existing config ({error}); left untouched"),
                ));
            }
        },
    };

    // A fresh `mcp_servers` is implicit so a first registration renders one
    // `[mcp_servers.medulla]` header rather than a bare `[mcp_servers]` above it.
    if document.get("mcp_servers").is_none() {
        let mut table = Table::new();
        table.set_implicit(true);
        document.insert("mcp_servers", Item::Table(table));
    }
    let Some(servers) = document["mcp_servers"].as_table_like_mut() else {
        return Ok(skipped(
            SkillTarget::Codex,
            &path,
            "`mcp_servers` exists but is not a table; fix it and re-run".to_string(),
        ));
    };

    if codex_entry_matches(servers.get(SERVER_NAME), opts) {
        return Ok(outcome(SkillTarget::Codex, path, "unchanged"));
    }
    let action = if existing.is_some() {
        "updated"
    } else {
        "created"
    };
    write_codex_entry(servers, opts);

    if !opts.dry_run {
        write_new_file(&path, &document.to_string())?;
    }
    Ok(outcome(SkillTarget::Codex, path, action))
}

/// Whether `item` already says everything [`write_codex_entry`] would write.
///
/// Compared by meaning, not by rendering: an operator who spelled the entry as
/// an inline table, or `env` as a sub-table, is saying the same thing and must
/// not be reformatted into our spelling. Keys we do not write are ignored rather
/// than counted as a difference — see [`write_codex_entry`] for what is ours.
fn codex_entry_matches(item: Option<&Item>, opts: &RegistrationOptions) -> bool {
    let Some(entry) = item.and_then(Item::as_table_like) else {
        return false;
    };
    command_matches(entry, opts) && args_matches(entry) && env_matches(entry, opts)
}

/// Whether the entry already points at this binary.
fn command_matches(entry: &dyn TableLike, opts: &RegistrationOptions) -> bool {
    entry.get("command").and_then(Item::as_str) == Some(&exe_string(opts))
}

/// Whether the entry's argv is exactly `["mcp"]`.
fn args_matches(entry: &dyn TableLike) -> bool {
    entry
        .get("args")
        .and_then(Item::as_array)
        .is_some_and(|args| {
            args.len() == 1 && args.get(0).and_then(TomlValue::as_str) == Some(MCP_ARG)
        })
}

/// Whether the entry's env already selects this tool mode.
///
/// Other variables in the same table are not our business, so they are not read
/// here and not written below.
fn env_matches(entry: &dyn TableLike, opts: &RegistrationOptions) -> bool {
    entry
        .get("env")
        .and_then(Item::as_table_like)
        .is_some_and(|env| env.get(TOOLS_ENV).and_then(Item::as_str) == Some(&opts.tools_mode))
}

/// Writes `[mcp_servers.medulla]` into `servers`, touching as little as possible.
///
/// Three keys are ours and are corrected: `command` and `args`, which *are* the
/// process this registration promises the harness, and the [`TOOLS_ENV`]
/// variable, which is the tool surface the operator chose. Everything else the
/// operator put under our table — a `startup_timeout_sec`, another environment
/// variable — is theirs and is left alone.
///
/// Even those three are only assigned when they do not already mean the right
/// thing, because assigning replaces the key's value and any trailing comment on
/// its line. A registration that only moved the binary therefore leaves the rest
/// of the table, and the table's position in the operator's file, as it was.
fn write_codex_entry(servers: &mut dyn TableLike, opts: &RegistrationOptions) {
    if !servers
        .get(SERVER_NAME)
        .is_some_and(|item| item.is_table_like())
    {
        servers.insert(SERVER_NAME, Item::Table(Table::new()));
    }
    let Some(entry) = servers
        .get_mut(SERVER_NAME)
        .and_then(Item::as_table_like_mut)
    else {
        // Unreachable: the branch above just made it a table.
        return;
    };

    if !command_matches(entry, opts) {
        entry.insert("command", toml_edit::value(exe_string(opts)));
    }
    if !args_matches(entry) {
        let mut args = Array::new();
        args.push(MCP_ARG);
        entry.insert("args", toml_edit::value(args));
    }
    if !env_matches(entry, opts) {
        write_tools_env(entry, opts);
    }
}

/// Sets [`TOOLS_ENV`] inside the entry's `env`, creating the table if needed.
///
/// Written into the operator's existing table rather than over it, so a session
/// they gave another variable keeps it when the tool mode changes.
fn write_tools_env(entry: &mut dyn TableLike, opts: &RegistrationOptions) {
    if !entry.get("env").is_some_and(|item| item.is_table_like()) {
        entry.insert("env", toml_edit::value(InlineTable::new()));
    }
    if let Some(env) = entry.get_mut("env").and_then(Item::as_table_like_mut) {
        env.insert(TOOLS_ENV, toml_edit::value(opts.tools_mode.clone()));
    }
}

/// The absolute binary path as it is written into a config.
fn exe_string(opts: &RegistrationOptions) -> String {
    opts.exe.display().to_string()
}

/// A written-file outcome.
fn outcome(target: SkillTarget, path: PathBuf, action: &str) -> RegistrationOutcome {
    RegistrationOutcome {
        target,
        path: Some(path),
        action: action.to_string(),
        manual_command: None,
    }
}

/// What was found at a config path.
///
/// Three cases rather than two, because "exists but we cannot read it as text"
/// must not be an error: registration runs *after* the skills have been
/// written, so failing here would leave the operator with installed skills, no
/// report, and — under `--json` — nothing on stdout to parse. It is reported as
/// `skipped` for the same reason a malformed config is.
enum Existing {
    /// No file at the path: this is a first registration.
    Absent,
    /// A file that is not valid UTF-8. Left alone; never parsed, never written.
    Unreadable,
    /// The file's contents.
    Text(String),
}

/// Reads `path`, distinguishing absent from present-but-undecodable so a first
/// registration, an update, and a file we must not touch stay tellable apart.
///
/// Bytes rather than `fs::read_to_string`, mirroring `install::read_existing`:
/// a config that is not UTF-8 is something we did not write and cannot safely
/// rewrite.
fn read_if_present(path: &Path) -> io::Result<Existing> {
    match fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8(bytes).map_or(Existing::Unreadable, Existing::Text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Existing::Absent),
        Err(error) => Err(error),
    }
}

/// Writes `body` to `path`, creating parent directories as needed.
fn write_new_file(path: &Path, body: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)
}
