//! Which hosts this machine runs, resolved from config alone.
//!
//! A host is a machine with a bus address; `[host]` declares the primary one and
//! each `[[hosts]]` entry another beside it. Both the process that *starts* them
//! and the UI that *lists* them need the same answer to "what address will this
//! section bind, and what should it be called" — and they must not derive it
//! twice, because a UI that disagrees with the binder would file a running local
//! host under a remote one and quietly show it as read-only.
//!
//! Resolution is deliberately config-only: it holds before anything starts,
//! which is the case the Hosts tab needs (declared agents on a host that is not
//! running are still that host's agents) and the case the roster clean-up needs
//! (recognising remembered local entries when nothing started at all).

use super::HostSection;

/// One host this machine declares, as the UI and the starter both see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalHostRef {
    /// The bus address it binds — the `hostId` its agents are declared under.
    pub id: String,
    /// What to call it on screen.
    pub name: String,
    /// The directory it works in, as configured. Blank means "wherever medulla
    /// was launched", which only the running host can resolve.
    pub workspace: String,
    /// Whether this is the `[host]` section rather than an `[[hosts]]` entry.
    pub primary: bool,
}

/// Every host this machine declares: the primary first, then the extras in
/// declaration order.
///
/// Addresses come from [`local_host_address`], names from [`local_host_name`].
pub fn local_hosts(primary: &HostSection, extras: &[HostSection]) -> Vec<LocalHostRef> {
    std::iter::once(LocalHostRef {
        id: primary.effective_address(),
        name: local_host_name(primary, &primary.workspace, true),
        workspace: primary.workspace.clone(),
        primary: true,
    })
    .chain(
        extras
            .iter()
            .enumerate()
            .map(|(index, extra)| LocalHostRef {
                id: local_host_address(extra, index),
                name: local_host_name(extra, &extra.workspace, false),
                workspace: extra.workspace.clone(),
                primary: false,
            }),
    )
    .collect()
}

/// The bus address for an extra host, derived from its name when it declared
/// none of its own.
///
/// Two hosts cannot share an address — the second `bind` fails — so an operator
/// who adds `[[hosts]]` without thinking about addressing would otherwise get
/// one working host and one startup error. Deriving from the name means the
/// field is optional in the common case and explicit when it matters.
///
/// The section default counts as unchosen, not as a choice: `[[hosts]]` shares
/// [`HostSection`], so an entry that names no address inherits the primary's.
/// An operator who *typed* the primary's address has made the same mistake, so
/// both are treated the same way.
pub fn local_host_address(config: &HostSection, fallback_index: usize) -> String {
    let chosen = config.address.trim();
    let chosen = if chosen == HostSection::default().address {
        ""
    } else {
        chosen
    };
    match chosen {
        "" => {
            let slug = slug_of(&config.name);
            if slug.is_empty() {
                format!("local-host-{}", fallback_index + 1)
            } else {
                format!("local-{slug}")
            }
        }
        value => value.to_string(),
    }
}

/// What to call a host that named itself nothing.
///
/// The primary is "this device" — it is the machine the operator is looking at.
/// An extra is named for the directory it works in, because that is the only
/// thing distinguishing it from the primary. `workspace` is the *resolved*
/// directory where the caller has one; the configured value is the honest
/// fallback before anything has started.
pub fn local_host_name(config: &HostSection, workspace: &str, primary: bool) -> String {
    match config.name.trim() {
        "" if primary => "this device".to_string(),
        "" => std::path::Path::new(workspace)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| workspace.to_string()),
        value => value.to_string(),
    }
}

/// A lowercase, hyphenated form of `name`, safe to use as a bus address.
fn slug_of(name: &str) -> String {
    let mut out = String::new();
    let mut hyphen = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            hyphen = false;
        } else if !out.is_empty() && !hyphen {
            out.push('-');
            hyphen = true;
        }
    }
    out.trim_end_matches('-').to_string()
}
