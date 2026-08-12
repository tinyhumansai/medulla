//! Hook delivery for the ACP transport, where Medulla never sees the harness's
//! own argv.
//!
//! Every other spawn door installs hooks by adding to the coding CLI's command
//! line. ACP dispatch has no such seam: Medulla spawns an ACP *server*
//! (`@agentclientprotocol/claude-agent-acp`, `codex-acp`, `opencode acp`) and
//! that server spawns the harness, so [`super::launch_args`]' argv has nowhere
//! to go. Until this module existed the whole transport ran with none of the
//! operator's hooks, and said so only in a `tracing::warn` nobody watching a
//! workflow would see.
//!
//! Claude carries hooks through its session metadata. Codex's ACP app-server
//! executes no lifecycle hooks itself, so its `PostToolUse` hooks run *locally*
//! in Medulla's own process after each completed tool call (see [`runner`]),
//! observation-only. Attribution is already applied through the inherited
//! `prepare-commit-msg` environment.

use serde_json::{json, Map, Value};

use crate::protocol::HarnessProvider;

use super::types::HooksConfig;

mod runner;
mod types;

#[cfg(test)]
mod tests;

pub use runner::run_post_tool_use;
pub use types::AcpDelivery;

/// Build the ACP-transport delivery installing `hooks` into `provider`.
///
/// Pure, and unlike [`super::launch_args`] it stays that way: the one thing
/// installation writes on the CLI path is Codex's trust store, and this
/// transport delivers nothing to Codex to trust.
///
/// Returns an empty delivery when no declared hook applies to `provider`.
/// Providers this transport cannot install into report that through
/// [`AcpDelivery::notes`] rather than failing or staying silent.
pub fn delivery(provider: HarnessProvider, hooks: &HooksConfig) -> AcpDelivery {
    let mut delivery = AcpDelivery {
        notes: super::hook_injection(provider, hooks).notes(),
        ..AcpDelivery::default()
    };
    let applicable = hooks.for_provider(provider);
    if applicable.is_empty() {
        return delivery;
    }
    let document = super::native::hook_document(&applicable);
    match provider {
        HarnessProvider::Claude => {
            delivery.session_meta = Some(claude_session_meta(&document));
        }
        HarnessProvider::Codex => {
            delivery.local_post_tool_use = applicable
                .into_iter()
                .filter(|hook| hook.event == super::types::HookEvent::PostToolUse)
                .cloned()
                .collect();
            let unsupported =
                hooks.for_provider(provider).len() - delivery.local_post_tool_use.len();
            if unsupported > 0 {
                delivery.notes.push(format!(
                    "{unsupported} hook(s) are not installed for this Codex ACP session: \\
                     `codex app-server` does not run lifecycle hooks other than Medulla's \\
                     PostToolUse fallback."
                ));
            }
            // An operator hook delivered through the fallback behaves
            // differently from a directly-spawned Codex hook: ACP gives the
            // client no way to amend a tool call it did not execute, so stdout
            // feedback is discarded. State that instead of letting the hook
            // look identical to the direct path. Medulla's own built-ins are
            // observation-only by design and need no note.
            if delivery
                .local_post_tool_use
                .iter()
                .any(|hook| !hook.builtin)
            {
                delivery.notes.push(
                    "a non-Medulla PostToolUse hook is run locally for this Codex ACP \
                     session, observation-only: the fallback cannot forward the hook's \
                     stdout decision into the session, because ACP gives the client no \
                     way to amend a tool call it did not execute."
                        .to_string(),
                );
            }
        }
        // Unreachable in practice: `HooksConfig::for_provider` filters on
        // `HookEvent::supported_by`, which is false for every event on both, so
        // `applicable` is already empty above — and those hooks are already
        // named in `notes` as dropped. Kept explicit rather than as a catch-all
        // so that adapting either provider makes this arm a compile error
        // instead of a silent no-op.
        HarnessProvider::Opencode | HarnessProvider::Openhuman => {}
    }
    delivery
}

/// The `_meta` object carrying `document` to `claude-agent-acp` as the Claude
/// Agent SDK's `settings` option.
///
/// Nested under `claudeCode.options` because that is where the ACP server looks;
/// the ACP spec reserves `_meta` for exactly this kind of implementation-specific
/// passenger, and a server that does not recognise the key ignores it.
fn claude_session_meta(document: &Value) -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert(
        "claudeCode".to_string(),
        json!({
            "options": {
                "settings": { "hooks": document },
            },
        }),
    );
    meta
}
