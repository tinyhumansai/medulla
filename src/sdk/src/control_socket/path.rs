//! Where the control socket lives, and whether it can be bound.
//!
//! Two problems this module exists to solve, both of which fail obscurely if
//! ignored:
//!
//! * **`sun_path` is short.** A unix socket address holds 104 bytes on macOS and
//!   108 on Linux, and an account-scoped path under a long `$HOME` overflows it.
//!   The kernel answers with `EINVAL`, which reads as a bug in the caller rather
//!   than as "your path is too long", so the fallback here is mandatory rather
//!   than defensive.
//! * **A socket file outlives its process.** A crashed instance leaves one
//!   behind, and binding over it without checking would either fail forever or
//!   silently steal a live instance's address.
//!
//! AGENTS.md treats socket paths as untrusted configuration to be validated at
//! boundaries — this module is that boundary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The most bytes a socket path may occupy.
///
/// The smaller of the two platform limits (macOS's 104) less one for the NUL
/// terminator, used on every platform so a path that works on Linux is not a
/// surprise failure on a colleague's laptop.
const MAX_SOCKET_PATH_BYTES: usize = 103;

/// Environment variable naming an explicit control socket path.
pub const CONTROL_SOCKET_ENV: &str = "MEDULLA_CONTROL_SOCKET";

/// Why a control socket path cannot be used.
#[derive(Debug)]
#[non_exhaustive]
pub enum ControlSocketError {
    /// Something already exists at the path and it is not a socket.
    ///
    /// Never unlinked: a regular file here is somebody's data, and a control
    /// plane that deletes unfamiliar files to get its address is worse than one
    /// that does not start.
    NotASocket(PathBuf),
    /// A live instance already answers at this path.
    AlreadyBound(PathBuf),
    /// The directory holding the socket is writable by others.
    ///
    /// Checked because a socket's own mode is set *after* bind, leaving a window
    /// where it is world-accessible. A private parent closes that window
    /// structurally rather than racing it.
    InsecureParent(PathBuf),
    /// No candidate path fit within the platform's `sun_path` limit.
    NoViablePath,
    /// The filesystem refused an operation.
    Io(String),
}

impl std::fmt::Display for ControlSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlSocketError::NotASocket(p) => write!(
                f,
                "{} exists and is not a socket, so the control plane will not bind there; \
                 move or remove it, or set a different path",
                p.display()
            ),
            ControlSocketError::AlreadyBound(p) => write!(
                f,
                "another Medulla instance is already listening on {}",
                p.display()
            ),
            ControlSocketError::InsecureParent(p) => write!(
                f,
                "{} is writable by other users, so a control socket there could be replaced; \
                 tighten it to 0700",
                p.display()
            ),
            ControlSocketError::NoViablePath => write!(
                f,
                "no control socket path fits within the platform's socket address limit"
            ),
            ControlSocketError::Io(m) => write!(f, "control socket path unusable: {m}"),
        }
    }
}

impl std::error::Error for ControlSocketError {}

/// A short, stable, account-distinct token for a home directory.
///
/// Used to keep the fallback paths apart when the preferred one does not fit.
/// Hashing the home rather than the account id keeps two roots on one machine
/// distinct even when they hold the same account.
fn home_token(home: &Path) -> String {
    let digest = Sha256::digest(home.as_os_str().as_encoded_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether a path fits in a socket address.
fn fits(path: &Path) -> bool {
    path.as_os_str().as_encoded_bytes().len() <= MAX_SOCKET_PATH_BYTES
}

/// Resolve the control socket path for this account.
///
/// Precedence: the `MEDULLA_CONTROL_SOCKET` environment variable, then an
/// explicit `[mcp].socketPath`, then an account-scoped path under the Medulla
/// home. The home-derived default is deliberately not `$XDG_RUNTIME_DIR`: what
/// must scope this socket is the *Medulla account* (`<root>/<user id>`), not the
/// OS user, so two accounts on one laptop never share a fleet.
///
/// When the account-scoped path would overflow the platform's socket address
/// limit, falls back to a hashed short path under `$XDG_RUNTIME_DIR` and then
/// `$TMPDIR`, each inside a private per-account directory. An explicit path from
/// the environment or config is returned as given — an operator who names a path
/// gets that path, and a silent substitution would be worse than the bind error.
///
/// # Errors
///
/// [`ControlSocketError::NoViablePath`] when every candidate overflows.
pub fn control_socket_path(
    env: &HashMap<String, String>,
    configured: Option<&str>,
) -> Result<PathBuf, ControlSocketError> {
    if let Some(explicit) = env
        .get(CONTROL_SOCKET_ENV)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return Ok(PathBuf::from(explicit));
    }
    if let Some(explicit) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }

    let home = crate::home::medulla_home(env);
    let preferred = home.join("control.sock");
    if fits(&preferred) {
        return Ok(preferred);
    }

    let token = home_token(&home);
    if let Some(xdg) = env
        .get("XDG_RUNTIME_DIR")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let candidate = PathBuf::from(xdg)
            .join("medulla")
            .join(&token)
            .join("control.sock");
        if fits(&candidate) {
            return Ok(candidate);
        }
    }

    let tmp = env
        .get("TMPDIR")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("/tmp");
    let candidate = PathBuf::from(tmp)
        .join(format!("medulla-{token}"))
        .join("control.sock");
    if fits(&candidate) {
        return Ok(candidate);
    }

    Err(ControlSocketError::NoViablePath)
}

/// Create the socket's parent directory, private to this user when Medulla owns
/// the path.
///
/// A default path is tightened to `0700` on every call. An explicitly trusted
/// path preserves an existing parent's permissions, but a parent created for it
/// is still private.
#[cfg(unix)]
fn ensure_private_parent(path: &Path, trusted_path: bool) -> Result<(), ControlSocketError> {
    use std::os::unix::fs::PermissionsExt;

    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let existed = parent.exists();
    std::fs::create_dir_all(parent).map_err(|e| ControlSocketError::Io(e.to_string()))?;
    // An explicitly named path may deliberately live in a shared directory.
    // Never mutate permissions on a directory Medulla did not create and does
    // not own merely because the operator placed a socket beneath it.
    if trusted_path && existed {
        return Ok(());
    }
    let mut perms = std::fs::metadata(parent)
        .map_err(|e| ControlSocketError::Io(e.to_string()))?
        .permissions();
    if perms.mode() & 0o077 != 0 {
        perms.set_mode(0o700);
        std::fs::set_permissions(parent, perms)
            .map_err(|e| ControlSocketError::Io(e.to_string()))?;
    }
    Ok(())
}

/// Whether the socket's parent directory is writable by group or others.
///
/// A writable parent means another user could unlink our socket and bind their
/// own in its place, which no permission on the socket file itself prevents.
#[cfg(unix)]
fn parent_is_insecure(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Some(parent) = path.parent() else {
        return false;
    };
    std::fs::metadata(parent)
        .map(|meta| meta.permissions().mode() & 0o022 != 0)
        .unwrap_or(false)
}

/// Make `path` bindable, or say why it is not.
///
/// Creates or validates the parent directory, then decides what to do about
/// anything already at the path: nothing there is fine; a non-socket is a hard
/// error and is never unlinked; an existing socket is *probed* by connecting to
/// it, and only unlinked when nothing answers.
///
/// The probe is deliberate. A pid file can go stale in the other direction — a
/// reused pid reads as live — and needs its own locking to stay honest, whereas
/// connecting tests the exact property that matters: does anything answer here.
///
/// # Errors
///
/// [`ControlSocketError::AlreadyBound`] when a live instance holds the path,
/// [`ControlSocketError::NotASocket`] when something else occupies it, and
/// [`ControlSocketError::InsecureParent`] when the directory is world-writable
/// and `trusted_path` is false.
#[cfg(unix)]
pub async fn prepare_bind(path: &Path, trusted_path: bool) -> Result<(), ControlSocketError> {
    use std::os::unix::fs::FileTypeExt;

    ensure_private_parent(path, trusted_path)?;
    // An operator who named an explicit path may have good reasons for an
    // unusual directory; the default path is ours and must be private.
    if !trusted_path && parent_is_insecure(path) {
        return Err(ControlSocketError::InsecureParent(
            path.parent().unwrap_or(path).to_path_buf(),
        ));
    }

    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(ControlSocketError::Io(err.to_string())),
    };
    if !meta.file_type().is_socket() {
        return Err(ControlSocketError::NotASocket(path.to_path_buf()));
    }

    match tokio::time::timeout(
        std::time::Duration::from_millis(250),
        tokio::net::UnixStream::connect(path),
    )
    .await
    {
        // Somebody answered: the address is taken by a live instance.
        Ok(Ok(_stream)) => Err(ControlSocketError::AlreadyBound(path.to_path_buf())),
        // Nothing is listening — the owner died and left the file behind.
        Ok(Err(err))
            if matches!(
                err.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            std::fs::remove_file(path).map_err(|e| ControlSocketError::Io(e.to_string()))
        }
        // Any other error, including a timeout, is ambiguous. Refusing to bind
        // is the safe half of that: the cost is no control plane this run, where
        // guessing wrong steals a live instance's address.
        Ok(Err(err)) => Err(ControlSocketError::Io(err.to_string())),
        Err(_) => Err(ControlSocketError::AlreadyBound(path.to_path_buf())),
    }
}

/// Tighten a freshly bound socket to owner-only access.
///
/// Belt and braces beside the private parent directory: between `bind` and this
/// call the socket carries whatever the umask allowed, and the parent is what
/// actually closes that window.
#[cfg(unix)]
pub fn restrict_socket(path: &Path) -> Result<(), ControlSocketError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| ControlSocketError::Io(e.to_string()))
}
