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
use medulla::protocol::HarnessProvider;
use medulla::sessions::SessionClass;

use super::executor::PtySessionExecutor;
use super::pty::PtyManager;

mod basic;
mod control;
mod live;
mod plumbing;
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
    harness_with_env(sessions_dir, workspace, &[])
}

/// As [`harness`], with `extra` layered into the environment the executor is
/// *constructed* with — not merely into the map a test happens to hold
/// afterward. `PtySessionExecutor::new` clones its base environment once, at
/// construction, so mutating the map a caller was handed back has no effect on
/// what the executor actually spawns with; overrides that matter (a fake
/// harness binary path, a router's resolved secret) must go in before that
/// clone happens.
fn harness_with_env(
    sessions_dir: &std::path::Path,
    workspace: &str,
    extra: &[(&str, &str)],
) -> (PtySessionExecutor, HashMap<String, String>) {
    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    // Point transcript discovery at the temp dir, and the "codex" binary at sh.
    env.insert(
        "MEDULLA_CODEX_SESSIONS_DIR".to_string(),
        sessions_dir.to_string_lossy().into_owned(),
    );
    env.insert("MEDULLA_CODEX_BIN".to_string(), "/bin/sh".to_string());
    for (key, value) in extra {
        env.insert(key.to_string(), value.to_string());
    }
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
        hooks: medulla::harness_hooks::HooksConfig::default(),
        transport: Default::default(),
        conversation: conversation.to_string(),
        // The fixture maps its two shapes onto the classes the daemon gives
        // them: a named peer is a conversation (unbound, reuses that peer's
        // session), an empty one is unattributed work (bounded, its own
        // session). Deriving it *here* is fine and is not the bug that was
        // fixed — production code no longer infers the class, it is told;
        // this just spares every call site a third argument while keeping each
        // test asserting what it was written to assert.
        session_class: if conversation.is_empty() {
            SessionClass::Bounded
        } else {
            SessionClass::Unbound
        },
        resume_session_id: None,
        workspace_context: Default::default(),
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
        router: None,
        on_event: None,
        on_stdin: None,
        on_session: None,
        on_workspace_context: None,
        attribution: true,
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
    env.get("MEDULLA_CODEX_SESSIONS_DIR")
        .cloned()
        .unwrap_or_default()
}

fn dialog_harness_script() -> String {
    "printf '1. Yes, I trust this folder\\r\\n'; read line; sleep 30".to_string()
}
