//! The `[[remoteHosts]]` section: machines reached over SSH whose sessions run
//! there and render here.
//!
//! Deliberately separate from [`HostSection`](super::HostSection), which is a
//! host *on this machine* addressed on the device-local bus. The two share the
//! word "host" and nothing else: a local host has a bus address and no login, a
//! remote host has login details and no bus address. Folding them into one type
//! would make `address` mean two things and force every reader to ask which kind
//! it was holding.
//!
//! Every field is optional. The minimum useful entry is a `host`, because
//! everything else — the user, the port, the identity file, the far-side binary —
//! has a working default or is something `ssh` already knows from the operator's
//! own `~/.ssh/config`.

use serde::{Deserialize, Serialize};

/// One machine reachable over SSH, as `medulla.toml` declares it.
///
/// The transport this feeds is `medulla-link`'s direct path
/// ([`medulla_link::LinkPath::Direct`]): SSH is only the bootstrap that starts a
/// daemon on the far side and carries its key back, after which datagrams go
/// straight there.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteHostSection {
    /// Stable identifier for this host, used in session ids and the rail lane.
    ///
    /// Derived from [`name`](Self::name), and failing that from
    /// [`host`](Self::host), when left blank — so an operator who wrote only a
    /// hostname still gets a durable id rather than a positional one that would
    /// shift if they reordered the file.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// What to call it on screen. Empty falls back to [`host`](Self::host),
    /// because a machine with an address and nothing to call it still has an
    /// address worth showing.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The SSH destination: a hostname, an IP, or an alias from the operator's
    /// `~/.ssh/config`. An entry without one cannot be connected to and is
    /// dropped by [`remote_hosts`](crate::config::remote_hosts).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// Login user. Empty lets `ssh` decide, which is right when `~/.ssh/config`
    /// already says — repeating it here only creates somewhere for the two to
    /// disagree.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub user: String,
    /// SSH port. `0` means unstated, so `ssh` applies its own default and any
    /// `Port` in the operator's config still wins.
    pub port: u16,
    /// Path to a private key, passed as `-i`. Empty leaves key selection to
    /// `ssh` and its agent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub identity_file: String,
    /// Extra arguments appended to the `ssh` command verbatim, for the cases a
    /// fixed set of fields cannot cover — `ProxyJump`, a jump host, a `Match`
    /// block that needs forcing.
    ///
    /// Untrusted configuration by the same reasoning as `[[hooks]]`: `-o
    /// ProxyCommand=…` runs a command on *this* machine. See
    /// [`load`](crate::config::load_config), which refuses this whole section
    /// from a project-local layer for exactly that reason.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ssh_options: Vec<String>,
    /// The SSH client to run. Empty uses `ssh` from `PATH`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ssh_binary: String,
    /// How `medulla` is invoked on the far side. Empty uses `medulla` from the
    /// remote `PATH`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote_command: String,
    /// The UDP address to send datagrams to, when it differs from
    /// [`host`](Self::host).
    ///
    /// Needed where SSH reaches the machine by a path UDP cannot follow — a
    /// `ProxyJump` through a bastion being the usual case. Empty means "the same
    /// place SSH went", which is right whenever SSH went there directly.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub udp_host: String,
    /// Default directory sessions on this host start in.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace: String,
    /// Other directories worth offering on the picker's workspace step.
    ///
    /// A remote directory cannot be completed from here — there is no filesystem
    /// to walk — so what the operator lists is what the picker can offer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<String>,
    /// Whether this host is offered at all. A disabled entry is kept in the file
    /// but never listed, which is what an operator wants for a machine that is
    /// away rather than gone.
    #[serde(default = "super::d_true")]
    pub enabled: bool,
}
