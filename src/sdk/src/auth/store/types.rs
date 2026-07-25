//! Data types for the `store` module.
#[allow(unused_imports)]
use super::*;
/// A JSON credential file (`{"baseUrl","jwt"}`) at a fixed path.
///
/// The default location is `<medulla_home>/credentials.json`; tests inject an
/// explicit path. On unix the file is written mode `0600`. A missing or corrupt
/// file is treated as "no credentials". For backward compatibility, reads fall
/// back to the retired `<config-dir>/medulla/credentials.json` location.
#[derive(Debug, Clone)]
pub struct CredentialStore {
    pub(super) path: PathBuf,
}
