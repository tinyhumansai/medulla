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

/// Exclusive claim spanning stale-socket inspection, reclamation, and bind.
///
/// Dropping releases the advisory filesystem lock. The lock file remains as a
/// harmless private sibling so every future starter contends on the same inode.
#[cfg(unix)]
#[must_use = "hold this guard until the socket has been bound"]
pub struct BindGuard {
    file: std::fs::File,
}

#[cfg(unix)]
impl Drop for BindGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

/// The most bytes a socket path may occupy.
///
/// The smaller of the two platform limits (macOS's 104) less one for the NUL
/// terminator, used on every platform so a path that works on Linux is not a
/// surprise failure on a colleague's laptop.
const MAX_SOCKET_PATH_BYTES: usize = 103;

/// How long startup waits for another process preparing the same socket.
#[cfg(unix)]
const BIND_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Whether an existing bind lock belongs to this OS user.
#[cfg(unix)]
pub(super) fn trusted_lock_owner(owner_uid: u32, effective_uid: u32) -> bool {
    owner_uid == effective_uid
}

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
/// `$TMPDIR`, each inside a private per-account directory. Explicit relative
/// paths are anchored to this process's current directory before they are
/// shared with child processes, whose own working directory may differ.
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
        return absolute_override(explicit);
    }
    if let Some(explicit) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        return absolute_override(explicit);
    }

    let home = absolute_path(crate::home::medulla_home(env))?;
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
        let candidate = absolute_path(
            PathBuf::from(xdg)
                .join("medulla")
                .join(&token)
                .join("control.sock"),
        )?;
        if fits(&candidate) {
            return Ok(candidate);
        }
    }

    let tmp = env
        .get("TMPDIR")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("/tmp");
    let candidate = absolute_path(
        PathBuf::from(tmp)
            .join(format!("medulla-{token}"))
            .join("control.sock"),
    )?;
    if fits(&candidate) {
        return Ok(candidate);
    }

    Err(ControlSocketError::NoViablePath)
}

/// Anchor an operator-supplied relative path before it crosses a process boundary.
fn absolute_override(explicit: &str) -> Result<PathBuf, ControlSocketError> {
    let path = absolute_path(PathBuf::from(explicit))?;
    if fits(&path) {
        Ok(path)
    } else {
        Err(ControlSocketError::NoViablePath)
    }
}

/// Anchor any relative socket path before it crosses a process boundary.
fn absolute_path(path: PathBuf) -> Result<PathBuf, ControlSocketError> {
    // A rooted slash path is the Unix spelling accepted by the runtime. Treat
    // it as anchored in cross-platform unit tests too, even though Windows
    // requires a drive prefix for `is_absolute` to return true.
    if path.is_absolute() || path.has_root() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|cwd| absolute_path_from(path, &cwd))
        .map_err(|error| ControlSocketError::Io(error.to_string()))
}

/// Anchor a relative path to a known working directory.
///
/// Keeping this operation separate from [`std::env::current_dir`] lets tests
/// exercise the process-boundary guarantee without depending on the checkout's
/// own path length.
pub(super) fn absolute_path_from(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() || path.has_root() {
        path
    } else {
        cwd.join(path)
    }
}

/// Create the socket's parent directory, private to this user when newly made.
///
/// Existing directories are never mutated, regardless of how the path was
/// selected. They may be shared deliberately or reached through a symlink;
/// security validation after this call either accepts their current mode or
/// rejects it without changing operator-owned filesystem state.
#[cfg(unix)]
fn ensure_private_parent(path: &Path) -> Result<(), ControlSocketError> {
    use std::os::unix::fs::PermissionsExt;

    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let existed = parent.exists();
    std::fs::create_dir_all(parent).map_err(|e| ControlSocketError::Io(e.to_string()))?;
    // Never mutate permissions on a directory Medulla did not create. This is
    // independent of whether the path was explicit: a derived MEDULLA_HOME can
    // also name an existing shared directory or a symlink to one.
    if existed {
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

/// The nearest directory through which another user could replace the socket.
///
/// Every ancestor matters: even when the immediate parent is private, a
/// writable grandparent could replace that entire directory and recreate the
/// socket path. A sticky directory such as `/tmp` is the exception: the kernel
/// permits its users to remove only entries they own.
#[cfg(unix)]
fn insecure_ancestor(path: &Path) -> Result<Option<PathBuf>, ControlSocketError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| ControlSocketError::Io(error.to_string()))?
            .join(path)
    };
    let Some(parent) = absolute.parent() else {
        return Ok(None);
    };
    // Follow the complete parent path once before walking upward. Calling
    // `metadata` on lexical ancestors follows only the ancestor currently
    // inspected and can miss a writable directory behind an earlier symlink.
    let resolved_parent =
        std::fs::canonicalize(parent).map_err(|error| ControlSocketError::Io(error.to_string()))?;
    for ancestor in resolved_parent.ancestors() {
        let meta = std::fs::metadata(ancestor)
            .map_err(|error| ControlSocketError::Io(error.to_string()))?;
        let mode = meta.permissions().mode();
        if mode & 0o022 != 0
            && (mode & 0o1000 == 0 || !trusted_sticky_owner(meta.uid(), effective_uid()))
        {
            return Ok(Some(ancestor.to_path_buf()));
        }
    }
    Ok(None)
}

/// Whether a sticky directory's owner is allowed to control this socket.
///
/// Sticky mode prevents ordinary peers from replacing one another's entries,
/// but the directory owner remains allowed to do so. Only this process's user
/// and root are therefore trusted owners.
#[cfg(unix)]
pub(super) fn trusted_sticky_owner(owner_uid: u32, effective_uid: u32) -> bool {
    owner_uid == effective_uid || owner_uid == 0
}

/// The effective Unix user whose socket entry the directory must protect.
#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments and has no safety preconditions.
    unsafe { libc::geteuid() }
}

/// The stable filesystem identity used to guard stale-socket reclamation.
#[cfg(unix)]
fn socket_identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;

    (meta.dev(), meta.ino())
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
/// [`ControlSocketError::InsecureParent`] when another user could replace an
/// entry in the parent directory. Existing directory modes are always
/// preserved, but do not bypass this validation. The returned [`BindGuard`]
/// must be held until `UnixListener::bind` completes so another starter cannot
/// reclaim the same stale path between this function and bind.
#[cfg(unix)]
pub async fn prepare_bind(path: &Path) -> Result<BindGuard, ControlSocketError> {
    use std::os::unix::fs::FileTypeExt;

    ensure_private_parent(path)?;
    if let Some(ancestor) = insecure_ancestor(path)? {
        return Err(ControlSocketError::InsecureParent(ancestor));
    }
    let bind_guard = acquire_bind_guard(path).await?;

    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(bind_guard),
        Err(err) => return Err(ControlSocketError::Io(err.to_string())),
    };
    if !meta.file_type().is_socket() {
        return Err(ControlSocketError::NotASocket(path.to_path_buf()));
    }
    let probed_identity = socket_identity(&meta);

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
            // Another starter may have reclaimed and rebound this path while
            // our connect was in flight. Never unlink an inode we did not
            // actually probe.
            let current = match std::fs::symlink_metadata(path) {
                Ok(current) => current,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(bind_guard)
                }
                Err(error) => return Err(ControlSocketError::Io(error.to_string())),
            };
            if !current.file_type().is_socket() || socket_identity(&current) != probed_identity {
                return Err(ControlSocketError::AlreadyBound(path.to_path_buf()));
            }
            std::fs::remove_file(path)
                .map_err(|error| ControlSocketError::Io(error.to_string()))?;
            Ok(bind_guard)
        }
        // Any other error, including a timeout, is ambiguous. Refusing to bind
        // is the safe half of that: the cost is no control plane this run, where
        // guessing wrong steals a live instance's address.
        Ok(Err(err)) => Err(ControlSocketError::Io(err.to_string())),
        Err(_) => Err(ControlSocketError::AlreadyBound(path.to_path_buf())),
    }
}

/// Acquire the trusted sibling lock every starter uses for this socket path.
#[cfg(unix)]
async fn acquire_bind_guard(path: &Path) -> Result<BindGuard, ControlSocketError> {
    use fs2::FileExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let Some(parent) = path.parent() else {
        return Err(ControlSocketError::Io(
            "control socket has no parent directory".to_string(),
        ));
    };
    let mut lock_name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("control.sock"))
        .to_os_string();
    lock_name.push(".bind.lock");
    let lock_path = parent.join(lock_name);
    let opened_path = lock_path.clone();
    let file = tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&opened_path)
            .map_err(|error| ControlSocketError::Io(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| ControlSocketError::Io(error.to_string()))?;
        if !metadata.file_type().is_file() {
            return Err(ControlSocketError::Io(format!(
                "{} is not a regular bind lock",
                opened_path.display()
            )));
        }
        // A lock beside an explicit socket may live in a shared sticky
        // directory. Another user can create the predictable sibling first;
        // never wait on or trust their inode.
        let effective_uid = unsafe { libc::geteuid() };
        if !trusted_lock_owner(metadata.uid(), effective_uid) {
            return Err(ControlSocketError::Io(format!(
                "{} belongs to another user",
                opened_path.display()
            )));
        }
        let deadline = std::time::Instant::now() + BIND_LOCK_TIMEOUT;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(ControlSocketError::AlreadyBound(opened_path));
                }
                Err(error) => return Err(ControlSocketError::Io(error.to_string())),
            }
        }
        Ok(file)
    })
    .await
    .map_err(|error| ControlSocketError::Io(error.to_string()))??;
    Ok(BindGuard { file })
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
