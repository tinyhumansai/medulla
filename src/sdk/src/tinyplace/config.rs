//! CLI config-file model and endpoint resolution.
//!
//! The config file (`TINYPLACE_CONFIG`, else `~/.tinyplace/config.json`) holds
//! the endpoint, identity key, SIWS proof, and OpenHuman owner. This module only
//! reads and models it; it does not generate keys or write files. Environment
//! lookups are passed in (not read from `std::env`) so resolution is testable.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Fallback API endpoint when none is configured. Aliases the single
/// source of truth in [`crate::config`] so the prod tiny.place URL is not
/// duplicated here.
pub const DEFAULT_ENDPOINT: &str = crate::config::PROD_TINYPLACE_BASE_URL;

/// The persisted CLI config. JSON field names match the TypeScript SDK
/// (camelCase for the multi-word keys).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TinyplaceFileConfig {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub endpoint: Option<String>,
    #[serde(rename = "secretKey", skip_serializing_if = "Option::is_none", default)]
    pub secret_key: Option<String>,
    #[serde(rename = "siwsToken", skip_serializing_if = "Option::is_none", default)]
    pub siws_token: Option<String>,
    #[serde(
        rename = "openHumanOwner",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub open_human_owner: Option<String>,
}

/// The absolute config-file path: `TINYPLACE_CONFIG` when set, otherwise
/// `<home>/.tinyplace/config.json`.
pub fn config_path(env: &HashMap<String, String>, home_dir: &Path) -> PathBuf {
    match env.get("TINYPLACE_CONFIG") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => home_dir.join(".tinyplace").join("config.json"),
    }
}

/// Parse config JSON into a [`TinyplaceFileConfig`]. A non-object or a malformed
/// document yields an empty config rather than an error, keeping only the keys
/// this crate recognizes.
pub fn parse_config(contents: &str) -> TinyplaceFileConfig {
    let value: serde_json::Value = match serde_json::from_str(contents) {
        Ok(value) => value,
        Err(_) => return TinyplaceFileConfig::default(),
    };
    if !value.is_object() {
        return TinyplaceFileConfig::default();
    }
    serde_json::from_value(value).unwrap_or_default()
}

/// Load and parse the config at `path`. A missing file (or any read error) is
/// treated as an empty config; this never panics.
pub fn load_config(path: &Path) -> TinyplaceFileConfig {
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_config(&contents),
        Err(_) => TinyplaceFileConfig::default(),
    }
}

/// Persist `config` to `path` as pretty JSON, atomically (write a sibling temp
/// file, then rename over the target) and with `0600` permissions on Unix. The
/// parent directory is created if missing. Mirrors the tinyplace CLI config
/// model so the file stays interoperable with the SDK/CLI.
pub fn write_config(path: &Path, config: &TinyplaceFileConfig) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut json = serde_json::to_string_pretty(config)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    json.push('\n');

    let pid = std::process::id();
    let tmp = path.with_extension(format!("json.tmp.{pid}"));
    std::fs::write(&tmp, json.as_bytes())?;
    set_owner_only(&tmp)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Resolve the API endpoint. Order: `TINYPLACE_ENDPOINT` > `TINYPLACE_API_URL` >
/// `NEXT_PUBLIC_API_URL` > `config.endpoint` > the staging/prod default. The
/// default is [`DEFAULT_ENDPOINT`] (prod), or the staging tiny.place URL when
/// `MEDULLA_STAGING` is truthy.
pub fn resolve_endpoint(env: &HashMap<String, String>, config: &TinyplaceFileConfig) -> String {
    for key in [
        "TINYPLACE_ENDPOINT",
        "TINYPLACE_API_URL",
        "NEXT_PUBLIC_API_URL",
    ] {
        if let Some(value) = env.get(key).map(|v| v.trim()).filter(|v| !v.is_empty()) {
            return value.to_string();
        }
    }
    // Delegate the `config.endpoint` → staging/prod-default tail to the shared
    // resolver so the "explicit value else default" logic (and its trim policy)
    // lives in exactly one place.
    crate::config::resolve_tinyplace_base_url(env, config.endpoint.as_deref())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use crate::tinyplace::{
        config_path, load_config, parse_config, resolve_endpoint, TinyplaceFileConfig,
        DEFAULT_ENDPOINT,
    };

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn config_path_prefers_env_override() {
        let e = env(&[("TINYPLACE_CONFIG", "/custom/config.json")]);
        assert_eq!(
            config_path(&e, Path::new("/home/me")),
            PathBuf::from("/custom/config.json")
        );
    }

    #[test]
    fn config_path_defaults_to_home() {
        let e = env(&[]);
        assert_eq!(
            config_path(&e, Path::new("/home/me")),
            PathBuf::from("/home/me/.tinyplace/config.json")
        );
        // Empty override is ignored.
        let e2 = env(&[("TINYPLACE_CONFIG", "")]);
        assert_eq!(
            config_path(&e2, Path::new("/home/me")),
            PathBuf::from("/home/me/.tinyplace/config.json")
        );
    }

    #[test]
    fn parses_a_full_config() {
        let contents = r#"{
        "endpoint": "https://staging-api.tiny.place",
        "secretKey": "deadbeef",
        "siwsToken": "siws:abc",
        "openHumanOwner": "owner-addr",
        "ignored": true
    }"#;
        let config = parse_config(contents);
        assert_eq!(
            config.endpoint.as_deref(),
            Some("https://staging-api.tiny.place")
        );
        assert_eq!(config.secret_key.as_deref(), Some("deadbeef"));
        assert_eq!(config.siws_token.as_deref(), Some("siws:abc"));
        assert_eq!(config.open_human_owner.as_deref(), Some("owner-addr"));
    }

    #[test]
    fn parse_config_tolerates_junk() {
        assert_eq!(parse_config("not json"), TinyplaceFileConfig::default());
        assert_eq!(parse_config("[1,2,3]"), TinyplaceFileConfig::default());
        assert_eq!(parse_config("42"), TinyplaceFileConfig::default());
        assert_eq!(parse_config("{}"), TinyplaceFileConfig::default());
    }

    #[test]
    fn load_config_missing_file_is_empty() {
        let config = load_config(Path::new("/no/such/tinyplace/config.json"));
        assert_eq!(config, TinyplaceFileConfig::default());
    }

    #[test]
    fn load_config_reads_a_real_file() {
        let dir = std::env::temp_dir().join(format!("tinyplace-proto-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"endpoint":"https://x.example","secretKey":"ab"}"#,
        )
        .unwrap();
        let config = load_config(&path);
        assert_eq!(config.endpoint.as_deref(), Some("https://x.example"));
        assert_eq!(config.secret_key.as_deref(), Some("ab"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn round_trips_config_omitting_empty_fields() {
        let config = TinyplaceFileConfig {
            endpoint: Some("https://x".to_string()),
            secret_key: None,
            siws_token: None,
            open_human_owner: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(json, r#"{"endpoint":"https://x"}"#);
        assert_eq!(parse_config(&json), config);
    }

    #[test]
    fn endpoint_resolution_order() {
        let config = TinyplaceFileConfig {
            endpoint: Some("https://config-endpoint".to_string()),
            ..Default::default()
        };

        // TINYPLACE_ENDPOINT wins over everything.
        let e = env(&[
            ("TINYPLACE_ENDPOINT", "https://one"),
            ("TINYPLACE_API_URL", "https://two"),
            ("NEXT_PUBLIC_API_URL", "https://three"),
        ]);
        assert_eq!(resolve_endpoint(&e, &config), "https://one");

        // Then TINYPLACE_API_URL.
        let e = env(&[
            ("TINYPLACE_API_URL", "https://two"),
            ("NEXT_PUBLIC_API_URL", "https://three"),
        ]);
        assert_eq!(resolve_endpoint(&e, &config), "https://two");

        // Then NEXT_PUBLIC_API_URL.
        let e = env(&[("NEXT_PUBLIC_API_URL", "https://three")]);
        assert_eq!(resolve_endpoint(&e, &config), "https://three");

        // Then config.endpoint.
        let e = env(&[]);
        assert_eq!(resolve_endpoint(&e, &config), "https://config-endpoint");

        // Finally the default.
        assert_eq!(
            resolve_endpoint(&e, &TinyplaceFileConfig::default()),
            DEFAULT_ENDPOINT
        );
    }

    #[test]
    fn empty_env_values_are_skipped() {
        let config = TinyplaceFileConfig::default();
        let e = env(&[
            ("TINYPLACE_ENDPOINT", ""),
            ("TINYPLACE_API_URL", "https://real"),
        ]);
        assert_eq!(resolve_endpoint(&e, &config), "https://real");
    }
}
