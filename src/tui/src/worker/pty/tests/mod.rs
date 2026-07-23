//! Tests for the PTY session layer, split so no file exceeds the repo's
//! 500-line ceiling: [`session`] covers allocation, the reader thread, emulator
//! parsing, resize, input and reaping; [`identity`] which harnesses get a minted
//! session id and how one is learned back.
//!
//! These drive a real child on a real pseudo-terminal — `/bin/sh`, not a coding
//! agent, so they stay fast, offline, and deterministic while still exercising
//! the parts that actually break. The launch spec and the polling helpers are
//! here because both submodules use them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::tinyplace::HarnessProvider;

use super::manager::PtyManager;
use super::types::{LaunchSpec, PtyState};

mod identity;
mod session;

/// A spec that runs `sh -c <script>` on a pty.
fn sh(script: &str) -> LaunchSpec {
    let mut env = HashMap::new();
    // A pty child with no PATH cannot exec anything useful.
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    LaunchSpec {
        // Codex, not Claude: claude now gets a minted `--session-id`, which
        // `/bin/sh` would reject as an unknown option. Codex takes no preset id,
        // so its interactive argv is empty and the script is the whole command.
        provider: HarnessProvider::Codex,
        bin: "/bin/sh".to_string(),
        cwd: "/".to_string(),
        env,
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        label: "test".to_string(),
        session_id: None,
    }
}

/// Spin until `check` passes or the deadline expires.
///
/// The budget is deliberately far larger than the milliseconds these conditions
/// actually need. Real children on real ptys are at the mercy of machine load,
/// and a tight deadline turns "the box was busy" into a red test — which is
/// worse than useless, because it trains you to re-run rather than read.
fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out after 30s waiting for: {what}");
}

/// The whole screen as one string.
fn screen_text(manager: &PtyManager, id: &str) -> String {
    manager
        .screen_rows(id)
        .expect("the session has a screen")
        .cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| c.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
