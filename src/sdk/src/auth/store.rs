//! The on-disk credential store: a JSON `{"baseUrl","jwt"}` file at a fixed
//! path, with the read-only migration fallback to the retired config-dir
//! location.

use std::io;
use std::path::{Path, PathBuf};

use super::types::Credentials;

/// A JSON credential file (`{"baseUrl","jwt"}`) at a fixed path.
///
/// The default location is `<medulla_home>/credentials.json`; tests inject an
/// explicit path. On unix the file is written mode `0600`. A missing or corrupt
/// file is treated as "no credentials". For backward compatibility, reads fall
/// back to the retired `<config-dir>/medulla/credentials.json` location.
#[derive(Debug, Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    /// A store rooted at an explicit path (used by tests).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The default store under the Medulla home directory
    /// (`<home>/credentials.json`).
    pub fn at_home(home: &Path) -> Self {
        Self::new(home.join("credentials.json"))
    }

    /// The retired store under the OS config directory
    /// (`<config-dir>/medulla/credentials.json`), consulted only as a migration
    /// fallback when the home-based file is absent.
    pub fn legacy_config_dir_location() -> Option<Self> {
        dirs::config_dir().map(|d| Self::new(d.join("medulla").join("credentials.json")))
    }

    /// The backing file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load credentials, or `None` when the file is missing or corrupt.
    pub fn load(&self) -> Option<Credentials> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Load from this store, falling back to the retired config-dir location when
    /// this store has no file yet (read-only migration; nothing is moved).
    pub fn load_or_legacy(&self) -> Option<Credentials> {
        if let Some(creds) = self.load() {
            return Some(creds);
        }
        Self::legacy_config_dir_location()
            .filter(|legacy| legacy.path() != self.path())
            .and_then(|legacy| legacy.load())
    }

    /// Persist credentials, creating the parent directory and (on unix) tightening
    /// the file mode to `0600`.
    pub fn save(&self, creds: &Credentials) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(creds).map_err(io::Error::other)?;
        std::fs::write(&self.path, json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Remove any stored credentials. A missing file is not an error.
    pub fn clear(&self) -> io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}
