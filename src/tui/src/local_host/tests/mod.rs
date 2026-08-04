//! Unit tests for the local-host wiring: the on/off decision, how the `[host]`
//! section becomes start-up options, and what the hub is told to advertise.
//!
//! Split by responsibility: [`options`] covers config-to-options translation,
//! [`lifecycle`] starting and advertising hosts, [`declarations`] the declared
//! agents a host advertises, [`dispatch`] which executor a task reaches, and
//! [`extras`] additional hosts on one machine. The helpers shared by more than
//! one of them live here.

use std::collections::HashMap;

mod declarations;
mod dispatch;
mod extras;
mod lifecycle;
mod options;

/// A path that is guaranteed to exist and be executable on every platform.
///
/// The running test binary itself. `/bin/sh` was the obvious choice and the
/// wrong one: it does not exist on Windows, so detection found nothing there and
/// every test that needed an "installed" harness failed on that runner alone.
pub(super) fn installed_bin() -> String {
    std::env::current_exe()
        .expect("the test binary has a path")
        .to_string_lossy()
        .into_owned()
}

/// An env with exactly `claude` "installed", so detection is deterministic and
/// independent of what the machine running the tests actually has.
pub(super) fn env_with_only_claude() -> HashMap<String, String> {
    HashMap::from([
        ("PATH".to_string(), String::new()),
        ("TINYPLACE_CLAUDE_BIN".to_string(), installed_bin()),
    ])
}
