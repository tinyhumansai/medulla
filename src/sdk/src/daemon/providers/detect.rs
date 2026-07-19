//! Provider discovery and invocation shaping: which daemon providers exist, how
//! to resolve a provider's binary, whether a provider accepts mid-run stdin, and
//! how to build the one-shot headless argv for each provider.

use std::collections::HashMap;

use crate::tinyplace::HarnessProvider;

use super::types::ExistsOnPath;

/// Every daemon-supported provider.
pub const DAEMON_PROVIDERS: [HarnessProvider; 3] = [
    HarnessProvider::Claude,
    HarnessProvider::Codex,
    HarnessProvider::Opencode,
];

/// Resolve the binary name/path for a provider (env override wins). Delegates to
/// the central resolver ([`crate::tinyplace::env::provider_bin`]) so the
/// daemon and wrapper share one bin-override contract.
pub fn provider_bin(provider: HarnessProvider, env: &HashMap<String, String>) -> String {
    crate::tinyplace::env::provider_bin(provider, env)
}

/// Default lookup: a path-ish name is probed directly for `X_OK`, a bare name is
/// searched across `$PATH` entries.
pub fn make_path_lookup(env: &HashMap<String, String>) -> ExistsOnPath {
    let path = env.get("PATH").cloned().unwrap_or_default();
    let dirs: Vec<String> = path
        .split(path_separator())
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect();
    Box::new(move |bin: &str| {
        if bin.contains('/') || bin.contains('\\') {
            return is_executable(std::path::Path::new(bin));
        }
        dirs.iter()
            .any(|dir| is_executable(&std::path::Path::new(dir).join(bin)))
    })
}

#[cfg(windows)]
fn path_separator() -> char {
    ';'
}
#[cfg(not(windows))]
fn path_separator() -> char {
    ':'
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Which of the (optionally restricted) providers have a binary on PATH.
pub fn detect_providers(
    env: &HashMap<String, String>,
    only: Option<&[HarnessProvider]>,
    exists_on_path: Option<&ExistsOnPath>,
) -> Vec<HarnessProvider> {
    let owned_lookup;
    let lookup: &ExistsOnPath = match exists_on_path {
        Some(lookup) => lookup,
        None => {
            owned_lookup = make_path_lookup(env);
            &owned_lookup
        }
    };
    let candidates: &[HarnessProvider] = only.unwrap_or(&DAEMON_PROVIDERS);
    candidates
        .iter()
        .copied()
        .filter(|provider| lookup(&provider_bin(*provider, env)))
        .collect()
}

/// Build the argv for a one-shot headless run of `provider`.
pub fn build_run_args(
    provider: HarnessProvider,
    prompt: &str,
    model: Option<&str>,
    agent: Option<&str>,
    extra_args: &[String],
    skip_permissions: bool,
) -> Vec<String> {
    // A prompt beginning with "-" would be parsed as a flag by the provider (an
    // injection vector since task text is remote-controlled); neutralize with a
    // leading space, which the model sees as insignificant.
    let prompt = if prompt.starts_with('-') {
        format!(" {prompt}")
    } else {
        prompt.to_string()
    };
    let mut args: Vec<String> = Vec::new();
    match provider {
        HarnessProvider::Claude => {
            args.extend(["-p", "--output-format", "stream-json", "--verbose"].map(String::from));
            if skip_permissions {
                args.push("--dangerously-skip-permissions".to_string());
            }
            // Claude Code takes the session model via the long `--model` flag
            // (codex/opencode use `-m`).
            if let Some(model) = model {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            args.extend(extra_args.iter().cloned());
            args.push(prompt);
        }
        HarnessProvider::Codex => {
            args.push("exec".to_string());
            args.push("--json".to_string());
            if let Some(model) = model {
                args.push("-m".to_string());
                args.push(model.to_string());
            }
            args.extend(extra_args.iter().cloned());
            args.push(prompt);
        }
        HarnessProvider::Opencode => {
            args.push("run".to_string());
            if let Some(model) = model {
                args.push("-m".to_string());
                args.push(model.to_string());
            }
            args.push("--agent".to_string());
            args.push(agent.unwrap_or("build").to_string());
            args.push("--format".to_string());
            args.push("json".to_string());
            args.extend(extra_args.iter().cloned());
            args.push(prompt);
        }
    }
    args
}

/// Whether a provider accepts mid-run stdin input (`input` frames).
///
/// `opencode run` treats a non-TTY stdin as prompt content and blocks at
/// startup reading it until EOF; it has no interactive mid-run stdin channel.
/// Piping (and holding) its stdin open therefore deadlocks the run, so it gets
/// an immediate-EOF null stdin and `input` frames must be rejected up front.
/// Claude/Codex accept forwarded `input` frames over a live stdin pipe.
pub fn supports_stdin(provider: HarnessProvider) -> bool {
    provider != HarnessProvider::Opencode
}

/// The wire name for a provider.
pub fn provider_name(provider: HarnessProvider) -> &'static str {
    provider.as_str()
}
