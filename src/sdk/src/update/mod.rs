//! Release update checking and self-update.
//!
//! The published release workflow attaches a `latest.json` manifest to the
//! GitHub "latest" release, so `releases/latest/download/latest.json` always
//! serves the newest version's shape:
//!
//! ```json
//! {
//!   "version": "3.8.0",
//!   "tag": "v3.8.0",
//!   "pubDate": "2026-07-18T00:00:00Z",
//!   "notes": "https://github.com/tinyhumansai/medulla/releases/tag/v3.8.0",
//!   "platforms": {
//!     "aarch64-apple-darwin": { "url": "https://.../medulla-v3.8.0-aarch64-apple-darwin.tar.gz", "sha256": "<hex>" }
//!   }
//! }
//! ```
//!
//! The pure core (parsing, semver comparison, platform selection) is separated
//! from the thin IO (HTTP GET, download + extract + atomic replace) so it can be
//! unit-tested without a network or a real binary swap. The module is split by
//! responsibility: [`types`] holds the data model, [`check`] the IO-free core
//! plus the manifest fetch, and [`install`] the download/verify/swap side
//! effects and the `medulla update` entry point. All public items are
//! re-exported here so callers use `medulla::update::*`.

mod check;
mod install;
mod types;

#[cfg(test)]
mod tests;

pub use check::{
    bin_name, check_for_update, is_newer, parse_manifest, pick_platform, platform_key, sha256_hex,
    update_url,
};
pub use install::{backup_path, download_and_stage, exe_is_writable, install_binary, run_update};
pub use types::{Manifest, PlatformEntry, UpdateInfo, DEFAULT_UPDATE_URL};
