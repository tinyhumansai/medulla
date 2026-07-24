//! End-to-end tests for [`PtySessionExecutor`].
//!
//! These use a **fake harness**: a shell script that paints to its pty (so the
//! session is watchable, exactly like a real one) and appends records to a
//! transcript in the codex rollout dialect. That exercises the whole path —
//! spawn, inject, tail, fold, settle — with no coding agent installed and no
//! network, which is what makes it deterministic.
//!
//! Codex's dialect is used because it takes no preset session id, so the script
//! can own its transcript filename. The claude path differs only in which
//! records mean "done", and that fold is pinned in the SDK's own tests.

use std::collections::HashMap;
use std::time::Duration;

use medulla::daemon::providers::{Abort, RunTaskOptions};
use medulla::tinyplace::HarnessProvider;

use super::executor::PtySessionExecutor;
use super::pty::PtyManager;

mod basic;
mod sessions;

/// A fake harness on the default session id, for tests that run only one.
fn fake_harness_script(rollout: &str, cwd: &str, reply: &str) -> String {
    fake_harness_script_as(rollout, cwd, reply, "sess-fake-1")
}

/// As [`fake_harness_script`], with the harness session id stated.
///
/// Concurrent sessions must each claim their own: the tailer pins to the id it
/// learns from the rollout, so two sessions reporting the same one are
/// indistinguishable to it and both tails can settle on whichever rollout is
/// found first. Real codex sessions mint distinct ids; a fixture that does not
/// was testing something the product never sees.
fn fake_harness_script_as(rollout: &str, cwd: &str, reply: &str, session_id: &str) -> String {
    format!(
        r#"
read -r prompt
printf 'working on: %s\r\n' "$prompt"
printf '{{"type":"session_meta","payload":{{"session_id":"{session_id}","cwd":"{cwd}"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"t1"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"agent_message","message":"looking at it","phase":"main"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"t1","last_agent_message":"{reply}"}}}}\n' >> '{rollout}'
sleep 30
"#
    )
}

/// Build an executor whose sessions run `script` instead of a real harness.
fn harness(
    sessions_dir: &std::path::Path,
    workspace: &str,
) -> (PtySessionExecutor, HashMap<String, String>) {
    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    // Point transcript discovery at the temp dir, and the "codex" binary at sh.
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
        sessions_dir.to_string_lossy().into_owned(),
    );
    env.insert("TINYPLACE_CODEX_BIN".to_string(), "/bin/sh".to_string());
    let executor = PtySessionExecutor::new(PtyManager::new(), env.clone(), workspace.to_string());
    (executor, env)
}

fn options(
    env: &HashMap<String, String>,
    conversation: &str,
    script: &str,
    cwd: &str,
) -> RunTaskOptions {
    RunTaskOptions {
        conversation: conversation.to_string(),
        resume_session_id: None,
        provider: HarnessProvider::Codex,
        prompt: "ship the fix".to_string(),
        cwd: cwd.to_string(),
        env: env.clone(),
        timeout_ms: 30_000,
        model: None,
        agent: None,
        // The script *is* the fake harness; `-c` makes sh run it.
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        abort: Abort::new(),
        on_event: None,
        on_stdin: None,
    }
}

fn conversational_harness_script(rollout: &str, cwd: &str) -> String {
    format!(
        r#"
printf '{{"type":"session_meta","payload":{{"session_id":"sess-fake-1","cwd":"{cwd}"}}}}\n' >> '{rollout}'
turn=0
while read -r prompt; do
  turn=$((turn+1))
  printf 'working on: %s\r\n' "$prompt"
  printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"t'$turn'"}}}}\n' >> '{rollout}'
  printf '{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"t'$turn'","last_agent_message":"answer '$turn'"}}}}\n' >> '{rollout}'
done
"#
    )
}

fn cwd_of(env: &HashMap<String, String>) -> String {
    env.get("TINYPLACE_CODEX_SESSIONS_DIR")
        .cloned()
        .unwrap_or_default()
}

fn dialog_harness_script() -> String {
    "printf '1. Yes, I trust this folder\\r\\n'; read line; sleep 30".to_string()
}
