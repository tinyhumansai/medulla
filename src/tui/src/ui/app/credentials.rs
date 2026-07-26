//! Cached provider subscription and API-key presence for the Routing UI.

use std::path::PathBuf;

use super::types::{App, CredentialStatus, RP_HARNESSES};

/// Read credential presence without ever loading or retaining secret values.
pub(super) fn detect_credential_status() -> CredentialStatus {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    CredentialStatus {
        claude_subscription: home
            .as_ref()
            .is_some_and(|path| path.join(".claude/.credentials.json").is_file())
            || env_present("CLAUDE_CODE_OAUTH_TOKEN"),
        codex_subscription: home
            .as_ref()
            .is_some_and(|path| path.join(".codex/auth.json").is_file()),
        anthropic_api_key: env_present("ANTHROPIC_API_KEY"),
        openai_api_key: env_present("OPENAI_API_KEY"),
        openrouter_api_key: env_present("OPENROUTER_API_KEY"),
    }
}

impl App {
    /// Refresh cached credential presence only when Manage Keys is active.
    pub(super) fn refresh_credential_status_if_needed(&mut self) {
        if self.routing_index == RP_HARNESSES {
            self.credential_status = detect_credential_status();
        }
    }
}

/// Whether a credential environment variable is present and non-blank.
fn env_present(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
}
