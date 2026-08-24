//! Launching a harness CLI in its **interactive** mode on a pseudo-terminal.
//!
//! This is the difference that makes the worker TUI possible. The daemon's
//! headless path runs `claude -p --output-format stream-json`, which emits JSON
//! and paints nothing; there is no screen to show. Here the CLI is started the
//! way a human starts it — no `-p`, no `--output-format` — so it renders its own
//! full-screen interface, and the PTY gives us that interface as a byte stream
//! to parse.
//!
//! A tty is not optional: Codex refuses to start with `stdin is not a terminal`,
//! and both harnesses fall back to dumb line mode without one.

use std::collections::HashMap;

use medulla::protocol::HarnessProvider;

/// Hand a harness about to be launched here Medulla's own tools.
///
/// The PTY door has no `session/new` to carry an offer in, so the registration
/// goes on the child's argv and the grant into a file only its owner can read
/// (see [`medulla::mcp::attach`] — both of which this appends to the spawn the
/// caller is assembling. Returns the key the session's fleet grant was minted
/// under, for [`LaunchSpec::mcp_grant_session`] to carry to the reap that
/// gives it back.
///
/// Called at both doors — the executor's dispatched session and the operator's
/// hand-opened one — because a harness Medulla started is a harness Medulla
/// serves, whichever door it came through. Without it an operator sitting in a
/// Medulla-spawned Claude Code had skills telling the model to call
/// `workflow_run` and no such tool in the session.
///
/// `bin` is the executable the caller has already resolved and is about to
/// launch — passed rather than re-derived, because that is the one the trust
/// decision has to be about; see [`medulla::protocol::env::bin_is_overridden`].
///
/// `log`, when given, is where an operator learns *why* `fleet_*` did not
/// attach to a provider binary they overrode — see `attach_cli`'s own docs on
/// when that note actually fires. Routed to the log, never to the terminal:
/// this crate owns the alternate screen, and a stray line on stdout/stderr
/// corrupts whatever pane is drawn there.
///
/// Also mints the credential this session's built-in lifecycle hooks report
/// with (see [`medulla::mcp::local_hook_grant`]) and writes it
/// straight into `env`, since — unlike the fleet grant — it is safe for every
/// subprocess the harness starts to inherit: a hook-only grant can do nothing
/// but attribute a report to the session it already is. That is also why it
/// is minted regardless of provider, override, or whether `attach_cli` above
/// granted fleet access at all: every provider's hooks are wired to call back
/// into this same command, so withholding this credential the way the fleet
/// grant is withheld would leave those hooks unable to report even where
/// nothing about the launch is untrusted.
///
/// [`LaunchSpec::mcp_grant_session`]: super::types::LaunchSpec::mcp_grant_session
#[cfg(feature = "workflows")]
pub fn attach_mcp(
    provider: HarnessProvider,
    bin: &str,
    env: &mut HashMap<String, String>,
    extra_args: &mut Vec<String>,
    log: Option<&medulla::daemon::LogFn>,
) -> Option<String> {
    // Unique per launch: the key *is* the grant's identity, so two sessions
    // sharing one would share a capability and revoking either would revoke
    // both.
    let session = format!("pty-{}", uuid::Uuid::new_v4());
    let fleet_granted = medulla::mcp::attach_cli(provider, bin, &session, env, extra_args, log);
    let hook_granted = if let Some((socket, token)) = medulla::mcp::local_hook_grant(&session) {
        env.insert(
            medulla::control_socket::HOOK_SOCKET_ENV.to_string(),
            socket.to_string_lossy().into_owned(),
        );
        env.insert(medulla::control_socket::HOOK_GRANT_ENV.to_string(), token);
        true
    } else {
        false
    };
    // Either grant minted under `session` is enough reason for the caller to
    // revoke it when this launch ends: `revoke_session` drops every grant
    // recorded under the key, fleet and hook alike, in one call. Returning
    // `None` only when nothing at all was minted keeps a hook-only launch —
    // Codex, or an overridden Claude binary, where `attach_cli` grants no
    // fleet access — from leaking its hook grant past this session's own
    // reap.
    fleet_granted.or_else(|| hook_granted.then_some(session))
}

/// Without the engine compiled in there is no tool server to attach.
#[cfg(not(feature = "workflows"))]
pub fn attach_mcp(
    provider: HarnessProvider,
    bin: &str,
    env: &mut HashMap<String, String>,
    extra_args: &mut Vec<String>,
    log: Option<&medulla::daemon::LogFn>,
) -> Option<String> {
    let _ = (provider, bin, env, extra_args, log);
    None
}

/// Refresh Medulla's managed skills and point a harness about to be launched
/// here at them.
///
/// The tools arrive on their own (see [`attach_mcp`]), but nothing in a fresh
/// session knows *which* workflows exist or that `workflow_run` is how to start
/// one. That knowledge is a skill file under `<medulla home>/<harness>-skills/`
/// — a root Medulla owns, so the operator's own `~/.claude` is left alone — and
/// all that remains is telling the child to read it, which for Claude Code is
/// `--add-dir`.
///
/// The files are re-rendered from the workflow store here rather than only by
/// `medulla skills install`, because the store moves and the skills did not:
/// a workflow authored in the TUI, evolved over MCP, disabled, or deleted left
/// the managed directory describing a catalog that no longer existed, and the
/// only cure was remembering to re-run a command. Now the session sees the
/// store as it is at the moment it starts.
///
/// Called at both pty doors for the same reason [`attach_mcp`] is: the headless
/// executor already does this in `medulla::daemon::providers`, and a harness
/// started on a pty is no less Medulla-spawned than one started headless.
///
/// `env` is the child's environment, not the parent's: it is what resolves
/// `MEDULLA_HOME`/`HOME`, so a scratch-home session refreshes and reads that
/// home's skills. `cwd` is the session's working directory, which is what makes
/// a project-local workflow visible to a session opened inside that project.
/// Appends nothing for a provider with no such flag (everything but Claude
/// Code), so those argvs are unchanged.
#[cfg(feature = "workflows")]
pub fn attach_skills(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
    cwd: &std::path::Path,
    extra_args: &mut Vec<String>,
) {
    // Nothing to describe to a session that was served no tools: every one of
    // these skills instructs the model to call `workflow_run`, and a workflow
    // `agent` node is launched without it on purpose (see
    // [`medulla::harness_tools`]). Skills naming an absent tool are worse than
    // no skills — the session spends a turn discovering the gap.
    if medulla::harness_tools::withheld(env) {
        return;
    }
    extra_args.extend(medulla::workflows::skills::refresh_managed(
        provider, env, cwd,
    ));
}

/// Without the engine compiled in there are no workflows to have skills for.
#[cfg(not(feature = "workflows"))]
pub fn attach_skills(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
    cwd: &std::path::Path,
    extra_args: &mut Vec<String>,
) {
    let _ = (provider, env, cwd, extra_args);
}

/// The argv for an interactive (screen-painting) run of `provider`.
///
/// Deliberately minimal. Every flag the headless path adds — `-p`,
/// `--output-format`, `--verbose` — exists to *suppress* the interface we are
/// here to render, so none of them belong on this argv. A prompt is not passed
/// either: work arrives by typing into the PTY, exactly as a human would.
pub fn interactive_args(
    provider: HarnessProvider,
    session_id: Option<&str>,
    skip_permissions: bool,
    model: Option<&str>,
    extra: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if matches!(
        provider,
        HarnessProvider::Opencode | HarnessProvider::Openhuman
    ) {
        // OpenCode and OpenHuman need their TUI subcommand; Claude and Codex
        // paint by default when handed a tty.
        args.push("tui".to_string());
    }
    if skip_permissions {
        args.extend(bypass_flag(provider).iter().map(|f| f.to_string()));
    }
    // Only claude accepts a preset id. Handing one over turns transcript
    // attribution from a guess ("newest file in this directory") into a fact
    // ("the file named after this session"), which is what makes two sessions in
    // one repo safe.
    if let (HarnessProvider::Claude, Some(id)) = (provider, session_id) {
        args.push("--session-id".to_string());
        args.push(id.to_string());
    }
    // Same flag spellings as the headless argv builder
    // (`build_resumed_run_args`): claude takes the long form, codex/opencode the
    // short one. An unset model leaves the harness's own default alone.
    if let Some(model) = model {
        match provider {
            HarnessProvider::Claude => {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            HarnessProvider::Codex | HarnessProvider::Opencode => {
                args.push("-m".to_string());
                args.push(model.to_string());
            }
            // OpenHuman selects agents through its own shared configuration;
            // a coding-provider model override has no meaning for it, and a
            // shell has no model at all.
            HarnessProvider::Openhuman | HarnessProvider::Shell => {}
        }
    }
    args.extend(extra.iter().cloned());
    args
}

/// The flag that stops a provider asking permission before each tool call.
///
/// A watched session is still an *unattended* one: the operator is looking at a
/// pane, not sitting in it answering prompts, and a peer's task that stops on
/// "allow this command?" has silently hung until its timeout. The harnesses
/// spell it differently, and neither name is one to guess at — both are taken
/// from the installed CLIs' own `--help`.
///
/// `opencode` gets nothing: it is refused for delegated work anyway, since it
/// writes no transcript a turn's completion could be read from.
pub fn bypass_flag(provider: HarnessProvider) -> &'static [&'static str] {
    match provider {
        HarnessProvider::Claude => &["--dangerously-skip-permissions"],
        HarnessProvider::Codex => &["--dangerously-bypass-approvals-and-sandbox"],
        HarnessProvider::Opencode => &[],
        HarnessProvider::Openhuman => &[],
        // A shell asks no permission to begin with; there is nothing to bypass.
        HarnessProvider::Shell => &[],
    }
}

/// Whether a provider accepts a preset session id.
///
/// Codex does not: `codex resume <id>` takes an *existing* id, and there is no
/// flag to choose one for a fresh session. Its rollout records its own id on
/// line one instead, so attribution reads rather than dictates.
pub fn accepts_preset_session_id(provider: HarnessProvider) -> bool {
    provider == HarnessProvider::Claude
}

/// Mint a session id for a provider that accepts one.
pub fn mint_session_id(provider: HarnessProvider) -> Option<String> {
    accepts_preset_session_id(provider).then(|| uuid::Uuid::new_v4().to_string())
}

/// Whether a provider paints a full-screen interface worth embedding.
///
/// All three do when given a tty; this exists so the UI can say something
/// truthful if that ever stops being true for one of them.
pub fn paints_a_screen(provider: HarnessProvider) -> bool {
    matches!(
        provider,
        HarnessProvider::Claude
            | HarnessProvider::Codex
            | HarnessProvider::Opencode
            | HarnessProvider::Openhuman
            // Not a full-screen interface, but the same deal: it writes to a
            // tty, so the emulator has something to show.
            | HarnessProvider::Shell
    )
}

/// The keystrokes that submit a line of injected text to a harness.
///
/// A carriage return, not a newline: the child's line discipline is in raw mode
/// and its TUI reads `\r` as Enter. Sending `\n` types a literal newline into
/// the composer of both Claude Code and Codex instead of submitting.
pub fn submit_sequence() -> &'static [u8] {
    b"\r"
}

/// Bracketed-paste wrapping for injected text.
///
/// Peer prompts can be long and contain newlines. Typed raw, every embedded
/// newline submits a partial prompt. Bracketing tells the harness "this is one
/// paste", which both Claude Code and Codex honour by inserting it as a single
/// block.
pub fn bracket_paste(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
