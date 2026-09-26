//! Which shells this device can offer, and what to call them.
//!
//! A shell session is the one entry in the picker that is not a coding agent
//! (see [`medulla::protocol::HarnessProvider::Shell`]). It still needs the two
//! things every other entry has — a binary to run and a name to show — and
//! neither is a constant: the operator's shell is whatever `$SHELL` says, and
//! the alternatives are whatever is installed.
//!
//! So the list is probed rather than declared. Anything not on this machine is
//! simply not offered, which is the same contract the coding CLIs get from
//! `detect_providers`: the picker never shows a row that cannot start.

use std::collections::HashMap;

use medulla::protocol::HarnessProvider;

/// One shell the operator may open a session on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellChoice {
    /// The picker label — the shell's own name (`zsh`, `bash`, `fish`).
    pub name: String,
    /// The binary to launch: an absolute path when the environment named one,
    /// otherwise a bare name resolved against `PATH` at spawn.
    pub bin: String,
}

impl ShellChoice {
    /// Build a choice for `bin`, labelling it with the binary's own file name.
    ///
    /// The label is the *basename*, so `/usr/local/bin/zsh` and `/bin/zsh` both
    /// read as `zsh` — an absolute path in a picker row is noise, and the path
    /// is not what distinguishes the entries the operator is choosing between.
    fn new(bin: &str) -> Self {
        let name = bin
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(bin)
            .to_string();
        Self {
            name,
            bin: bin.to_string(),
        }
    }
}

/// The shells worth offering when the environment names none — ordered by how
/// likely an operator is to want one, not alphabetically.
const CANDIDATES: [&str; 4] = ["zsh", "bash", "fish", "sh"];

/// Every shell installed on this device, the operator's own first.
///
/// `$SHELL` (or a `MEDULLA_SHELL_BIN` pin) leads, because it is the shell whose
/// prompt, aliases, and history the operator actually has — opening anything
/// else by default would make a session in the TUI behave unlike the terminal
/// it was opened from. The rest follow so a host that pins `sh` still offers the
/// interactive shell someone would rather type in.
///
/// `exists` answers whether a binary can be executed — the same `PATH` probe the
/// daemon uses for coding CLIs, injected so this is testable without touching
/// the machine's real `PATH`. Duplicates are dropped by *name*, not by path, so
/// `$SHELL=/bin/zsh` does not produce a second `zsh` row for the one on `PATH`.
pub fn available(env: &HashMap<String, String>, exists: &dyn Fn(&str) -> bool) -> Vec<ShellChoice> {
    let configured = medulla::protocol::env::provider_bin(HarnessProvider::Shell, env);
    let mut choices: Vec<ShellChoice> = Vec::new();
    for bin in std::iter::once(configured.as_str()).chain(CANDIDATES) {
        let choice = ShellChoice::new(bin);
        if choices.iter().any(|seen| seen.name == choice.name) {
            continue;
        }
        if exists(bin) {
            choices.push(choice);
        }
    }
    choices
}
