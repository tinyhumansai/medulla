//! Enabling the hooks Medulla injects into Codex, and nothing else.
//!
//! # Why this exists
//!
//! Codex will not run a hook it has not been told to trust. It records every
//! hook it discovers in `$CODEX_HOME/config.toml` as
//!
//! ```toml
//! [hooks.state."<source>:<event>:<group>:<index>"]
//! trusted_hash = "sha256:…"
//! enabled = true
//! ```
//!
//! keyed by where the hook came from and where it sits in that source, carrying
//! a hash of the hook itself and a flag that starts out absent. Until the flag
//! is `true`, the hook is skipped — with nothing on stderr and a zero exit, so
//! an operator's only clue is that their hook never fired.
//!
//! Medulla delivers hooks as a `-c hooks=…` command-line override
//! ([`super::config_args`]) rather than by writing a file, which Codex keys
//! under the pseudo-source [`INJECTED_SOURCE`]. Those entries are therefore
//! *always* new to the trust store on a machine Medulla has not run Codex on,
//! and every hook the operator declared — plus Medulla's own reporting
//! built-ins — did nothing at all until someone opened the Codex TUI and
//! pressed `t` on the Hooks screen. That is not a step a headless box or a
//! dispatched agent can take.
//!
//! # What it is allowed to touch
//!
//! Only entries whose source is [`INJECTED_SOURCE`], and only for the events
//! this very spawn is injecting. Medulla speaks for the hooks Medulla installs;
//! it does not speak for a hook the checked-out repository ships in its own
//! `.codex/hooks.json`, or one the operator wrote into `$CODEX_HOME/hooks.json`
//! by hand. That is the distinction `--dangerously-bypass-hook-trust` cannot
//! make: the flag is invocation-wide, so using it to authorize Medulla's own
//! hooks would silently authorize the workspace's too.
//!
//! # Why the hash is never written
//!
//! [`enable`] flips `enabled` and touches nothing else. `trusted_hash` is
//! Codex's record of *what* was approved, and Medulla neither computes nor
//! forges it — writing a hash here would turn "trust the hooks Medulla
//! installs" into "trust whatever now sits at that position", which is the
//! blanket authorization this module exists to avoid.
//!
//! The consequence is a one-launch lag, and only for a hook Codex has never
//! seen: Codex registers it (with the correct hash, disabled) at session start,
//! and the next launch enables it. A hook whose text has not changed is already
//! recorded, so the steady state is that every Medulla spawn finds its hooks
//! enabled before Codex reads them. Should a *different* hook come to occupy a
//! position Codex has a hash for, Codex reports it as needing review and keeps
//! the old hash; that case still costs one interactive acceptance, deliberately.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use fs2::FileExt;
use tempfile::NamedTempFile;

use crate::harness_hooks::types::HookEvent;

/// The source Codex keys a `-c hooks=…` command-line override under.
///
/// Not a real path: Codex uses it for any hook that arrived as a config
/// override rather than from a file on disk. Within a Medulla-launched Codex
/// that is exactly Medulla's own injection and nothing else, which is what
/// makes it usable as the "ours" marker here.
pub const INJECTED_SOURCE: &str = "/<session-flags>/config.toml";

/// Codex's spelling of `event` in a `hooks.state` key: `PostToolUse` becomes
/// `post_tool_use`.
fn state_event(event: HookEvent) -> String {
    let name = event.as_str();
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.char_indices() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Whether `key` names an injected hook for one of `events`.
///
/// The key is `<source>:<event>:<group>:<index>`. Both the source and the event
/// must match: an injected hook for an event this spawn is not installing
/// belongs to some other Medulla launch's hook set, and enabling it here would
/// be approving a hook the operator has since removed.
fn is_ours(key: &str, events: &HashSet<String>) -> bool {
    let Some(rest) = key.strip_prefix(INJECTED_SOURCE) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(':') else {
        return false;
    };
    let mut parts = rest.split(':');
    let Some(event) = parts.next() else {
        return false;
    };
    // `<group>:<index>`, both numeric, and nothing after them.
    let numeric = parts.clone().count() == 2
        && parts.all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    numeric && events.contains(event)
}

/// Set `enabled = true` on every [`INJECTED_SOURCE`] entry for `events`.
///
/// Returns how many entries changed; zero when they were already enabled, when
/// Codex has not registered them yet, or when there is no config file at all —
/// none of which is an error. Errors only on a config that exists but cannot be
/// read or replaced.
///
/// The rewrite is textual and surgical: the file is Codex's, not Medulla's, and
/// round-tripping it through a TOML serializer would reformat and reorder an
/// operator's whole configuration to change one boolean. Every other line is
/// preserved byte for byte, and the replacement lands through a temporary file
/// and a rename so a Codex reading the config concurrently never sees half of
/// one. Concurrent launches serialize their read-modify-write cycle with an
/// advisory lock beside the resolved configuration, and the replacement keeps
/// the configuration's permissions and any user-facing symlink intact.
pub fn enable(config_path: &Path, events: &[HookEvent]) -> anyhow::Result<usize> {
    // Resolve the destination before making a sibling lock or temporary file.
    // Replacing `config_path` itself would replace a dotfile manager's symlink
    // rather than the configuration it manages.
    let config_path = match std::fs::canonicalize(config_path) {
        Ok(path) => path,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(anyhow::anyhow!(
                "Cannot resolve {}: {err}",
                config_path.display()
            ))
        }
    };
    let lock_path = config_path.with_extension("toml.medulla-trust.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|err| anyhow::anyhow!("Cannot open {}: {err}", lock_path.display()))?;
    lock.lock_exclusive()
        .map_err(|err| anyhow::anyhow!("Cannot lock {}: {err}", lock_path.display()))?;

    let text = match std::fs::read_to_string(&config_path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(anyhow::anyhow!(
                "Cannot read {}: {err}",
                config_path.display()
            ))
        }
    };

    let wanted: HashSet<String> = events.iter().map(|event| state_event(*event)).collect();
    let (rendered, changed) = rewrite(&text, &wanted);
    if changed == 0 {
        return Ok(0);
    }

    replace(&config_path, rendered.as_bytes())?;
    Ok(changed)
}

/// Atomically replace an existing configuration without widening its access.
///
/// `NamedTempFile::persist` uses an overwrite-capable move on Windows, unlike
/// `std::fs::rename`, while still being a same-directory atomic replacement on
/// Unix. The caller holds the adjacent advisory lock for the entire cycle.
fn replace(config_path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let permissions = std::fs::metadata(config_path)
        .map_err(|err| anyhow::anyhow!("Cannot inspect {}: {err}", config_path.display()))?
        .permissions();
    let parent = config_path.parent().ok_or_else(|| {
        anyhow::anyhow!("Cannot determine the parent of {}", config_path.display())
    })?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|err| anyhow::anyhow!("Cannot create a temporary config file: {err}"))?;
    temporary
        .write_all(contents)
        .map_err(|err| anyhow::anyhow!("Cannot write temporary config file: {err}"))?;
    temporary
        .as_file()
        .set_permissions(permissions)
        .map_err(|err| {
            anyhow::anyhow!(
                "Cannot preserve permissions on {}: {err}",
                config_path.display()
            )
        })?;
    temporary
        .persist(config_path)
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!("Cannot replace {}: {}", config_path.display(), err.error))
}

/// [`enable`]'s pure half: `text` with our entries enabled, and how many
/// changed.
///
/// Split out so the decision — which table headers are ours, and what a table
/// body missing the flag entirely should become — is testable without a
/// filesystem.
fn rewrite(text: &str, events: &HashSet<String>) -> (String, usize) {
    let mut out = String::with_capacity(text.len() + 64);
    let mut changed = 0;
    // Lines of the table currently being read, once it is one of ours. Held
    // rather than emitted because the flag may be missing altogether, in which
    // case it has to be appended before the next table starts.
    let mut pending: Option<Vec<&str>> = None;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            changed += flush(pending.take(), &mut out);
            if header_key(trimmed).is_some_and(|key| is_ours(&key, events)) {
                out.push_str(line);
                out.push('\n');
                pending = Some(Vec::new());
                continue;
            }
        }
        match pending.as_mut() {
            Some(body) => body.push(line),
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    changed += flush(pending.take(), &mut out);
    (out, changed)
}

/// Write one of our table bodies to `out`, enabled, and report whether that
/// changed anything.
///
/// Three shapes, all of which Codex produces: the flag already `true` (leave
/// it), the flag `false` (flip it in place, keeping its position among the
/// other keys), and no flag at all — the state Codex writes when it first
/// registers a hook — where it is appended after the table's last real key so
/// it does not land beyond a trailing blank line and read as belonging to
/// whatever comes next.
fn flush(body: Option<Vec<&str>>, out: &mut String) -> usize {
    let Some(body) = body else { return 0 };
    if body.iter().any(|line| flag(line) == Some(true)) {
        for line in body {
            out.push_str(line);
            out.push('\n');
        }
        return 0;
    }
    if body.iter().any(|line| flag(line) == Some(false)) {
        for line in body {
            match flag(line) {
                Some(false) => out.push_str("enabled = true\n"),
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        return 1;
    }
    let last_key = body.iter().rposition(|line| !line.trim().is_empty());
    for (index, line) in body.iter().enumerate() {
        out.push_str(line);
        out.push('\n');
        if Some(index) == last_key {
            out.push_str("enabled = true\n");
        }
    }
    if last_key.is_none() {
        out.push_str("enabled = true\n");
    }
    1
}

/// The quoted key inside a `[hooks.state."…"]` header, if the line is one.
fn header_key(line: &str) -> Option<String> {
    let rest = line.strip_prefix("[hooks.state.\"")?;
    let end = rest.find("\"]")?;
    Some(rest[..end].to_string())
}

/// `Some(value)` when `line` is an `enabled = <bool>` key.
fn flag(line: &str) -> Option<bool> {
    let rest = line.trim_start().strip_prefix("enabled")?;
    let rest = rest.trim_start().strip_prefix('=')?;
    match rest.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;
