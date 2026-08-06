//! `medulla hook <Event>`: the shim Medulla's own lifecycle hooks run.
//!
//! Installed into every harness Medulla launches (see
//! [`medulla::harness_hooks::builtin`]) and never run by hand. The harness
//! writes its native hook payload to this process's stdin; this reads it,
//! reduces it to one line, and files it against the control socket the spawn was
//! already handed.
//!
//! # It must not be able to hurt the session
//!
//! Three rules, all of them load-bearing because a hook runs *inside* an
//! operator's turn:
//!
//! - **Always exit zero.** A non-zero exit is how a hook tells Claude Code and
//!   Codex that something is wrong, and both surface it to the operator. Medulla
//!   failing to record its own bookkeeping is not the operator's problem.
//! - **Write nothing to stdout.** Both harnesses parse hook stdout as a decision
//!   protocol; a stray line there is a hook trying to steer the session.
//! - **Give up quickly.** No Medulla listening is the ordinary case — fleet
//!   tools off, a harness on a remote worker — and it must cost a moment, not a
//!   timeout.
//!
//! # What is sent
//!
//! A summary, never the payload. The payload holds prompt text, tool inputs, and
//! file contents; the summary is "used Edit" or "prompt submitted (412 chars)".
//! See [`medulla::harness_hooks::HookReport`] for why that boundary is here, at
//! the source, rather than at the log.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use medulla::control_socket::hook_grant_from_env;
use medulla::harness_hooks::HookEvent;

/// `medulla::harness_hooks::builtin`'s declared timeout for `event`'s
/// built-in, so the shim's own deadline can never exceed what the harness
/// will actually wait before it gives up on this process outright — a hard
/// kill, not the quiet, no-report exit the module docs promise. `SessionEnd`
/// is 3 seconds (Codex's own hook config refuses a longer one); every other
/// event is 5. Read from the SDK rather than mirrored here, so the two can
/// never drift apart.
fn declared_timeout(event: HookEvent) -> Duration {
    medulla::harness_hooks::builtin::declared_timeout(event)
}

/// Subtracted from [`declared_timeout`] before anything runs, so the shim's
/// own deadline always falls short of the harness's kill: a shim the harness
/// had to kill looks to Claude Code or Codex like a hook that hung, which is
/// exactly the failure mode "give up quickly" exists to avoid.
const HEADROOM: Duration = Duration::from_millis(300);

/// How long [`read_payload`] may spend at most, before whatever is left of
/// the shim's own deadline narrows it further. Keeps a harness that opens the
/// pipe and writes nothing from spending the *whole* deadline just reading,
/// with nothing left to file the report.
const READ_BUDGET: Duration = Duration::from_millis(500);

/// Run the shim for the event named in `args`.
///
/// Never returns an error: see the module docs. `args` is everything after the
/// `hook` subcommand, of which only the first entry — the canonical event name —
/// is read.
///
/// One deadline covers the whole run, from here to the report: previously the
/// stdin read and the report each had their own separate budget
/// (500ms + 3s = 3.5s worst case) against `SessionEnd`'s declared 3-second
/// timeout, so a slow read alone could run the shim past the point Codex kills
/// it rather than let it give up quietly (chatgpt-codex-connector P2 on PR
/// #192). Deriving both from one deadline means the total can never exceed
/// what was declared, whichever event this is.
pub(crate) async fn run_hook_cmd(args: &[String], env: &HashMap<String, String>) {
    let Some(event) = args.first().and_then(|name| HookEvent::from_wire(name)) else {
        return;
    };
    let deadline = Instant::now() + declared_timeout(event).saturating_sub(HEADROOM);
    let payload = read_payload(deadline);
    // Both are set together by the spawn that minted this session's hook-only
    // grant (see `medulla::mcp::attach::attach_mcp`'s doc comment for why this
    // is a plain environment variable, unlike the fleet grant the `medulla
    // mcp` subprocess gets); neither present means this harness was not
    // launched with a control plane to report to, which is a normal way to
    // run and not an error.
    let Some((socket, token)) = hook_grant_from_env(env) else {
        return;
    };

    #[cfg(unix)]
    {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let _ = tokio::time::timeout(remaining, report(&socket, &token, event, &payload)).await;
    }
    // The control socket is a unix socket; elsewhere there is nothing to report
    // to, and the shim is a no-op rather than a build error.
    #[cfg(not(unix))]
    let _ = (socket, token, payload);
}

/// File one report. Every failure is swallowed by the caller.
#[cfg(unix)]
async fn report(
    socket: &std::path::Path,
    token: &str,
    event: HookEvent,
    payload: &Value,
) -> Result<(), medulla::control_socket::ControlError> {
    let mut client = medulla::control_socket::client::ControlClient::connect(socket, token).await?;
    client
        .call(
            "hook.report",
            json!({
                "event": event.as_str(),
                "summary": summarize(event, payload),
                "cwd": payload.get("cwd").and_then(Value::as_str),
                "harnessSessionId": payload.get("session_id").and_then(Value::as_str),
            }),
        )
        .await?;
    Ok(())
}

/// Read the harness's payload from stdin, or `Null` when it sends none.
///
/// Bounded in both time and size: a harness that opens the pipe and writes
/// nothing must not park this process for the whole budget, and a payload
/// carrying a large tool result must not be held in memory to be summarized in
/// one line. The time bound is the smaller of [`READ_BUDGET`] and whatever is
/// left of `deadline` — the shim's one deadline for the whole run (see
/// [`run_hook_cmd`]) — so a read that is allowed to run right up to
/// `READ_BUDGET` can never by itself leave the report with no time left
/// against a shorter declared timeout such as `SessionEnd`'s.
fn read_payload(deadline: Instant) -> Value {
    /// Enough for the fields summarized below and the JSON around them.
    const MAX_PAYLOAD: u64 = 256 * 1024;

    let budget = READ_BUDGET.min(deadline.saturating_duration_since(Instant::now()));
    let (tx, rx) = std::sync::mpsc::channel();
    // Detached on purpose: if stdin never closes, this thread parks forever and
    // the process exits out from under it rather than waiting.
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = std::io::stdin()
            .lock()
            .take(MAX_PAYLOAD)
            .read_to_string(&mut text);
        let _ = tx.send(if read.is_ok() { text } else { String::new() });
    });
    rx.recv_timeout(budget)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// One operator-facing line for `event`, drawn from the fields the harnesses
/// agree on.
///
/// Claude Code and Codex both send `hook_event_name`, `session_id`, `cwd`, and —
/// for tool events — `tool_name`. Anything richer is per-harness and per-version,
/// so it is read defensively and skipped when absent: a report with a thin
/// summary is worth far more than one that did not arrive.
fn summarize(event: HookEvent, payload: &Value) -> String {
    sanitize(&summarize_raw(event, payload))
}

/// Strips control characters (including terminal escape/OSC sequences) from
/// harness-supplied text before it becomes a [`HookReport::summary`]. Every
/// path below folds arbitrary strings from the harness's own hook payload —
/// `tool_name`, `message` — into the summary, and that string is later
/// rendered straight into the operator's terminal, so it must never carry a
/// raw control byte.
fn sanitize(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

fn summarize_raw(event: HookEvent, payload: &Value) -> String {
    let field = |key: &str| payload.get(key).and_then(Value::as_str).unwrap_or("");
    match event {
        HookEvent::PostToolUse => match field("tool_name") {
            "" => "used a tool".to_string(),
            tool => format!("used {tool}"),
        },
        // The prompt itself stays in the harness; its size is the part that
        // says something about the turn.
        HookEvent::UserPromptSubmit => match payload.get("prompt").and_then(Value::as_str) {
            Some(prompt) => format!("prompt submitted ({} chars)", prompt.chars().count()),
            None => "prompt submitted".to_string(),
        },
        // The one event whose text is *for* the operator — it is what the
        // harness would have shown them — so it travels.
        HookEvent::Notification => match field("message") {
            "" => "notification".to_string(),
            message => message.to_string(),
        },
        HookEvent::Stop => "turn finished".to_string(),
        HookEvent::SubagentStart => "subagent started".to_string(),
        HookEvent::SubagentStop => "subagent finished".to_string(),
        HookEvent::SessionStart => match field("source") {
            "" => "session started".to_string(),
            source => format!("session started ({source})"),
        },
        HookEvent::SessionEnd => match field("reason") {
            "" => "session ended".to_string(),
            reason => format!("session ended ({reason})"),
        },
        HookEvent::PreCompact => "compacting the transcript".to_string(),
        HookEvent::PostCompact => "transcript compacted".to_string(),
        HookEvent::PreToolUse => match field("tool_name") {
            "" => "about to use a tool".to_string(),
            tool => format!("about to use {tool}"),
        },
        HookEvent::PermissionRequest => "asked for permission".to_string(),
    }
}

#[cfg(test)]
#[path = "hook_tests.rs"]
mod tests;
