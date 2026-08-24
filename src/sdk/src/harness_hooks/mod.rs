//! Standardized lifecycle hooks for Medulla-launched harnesses.
//!
//! An operator declares a hook once, in Medulla's own config or on the TUI's
//! Hosts → Hooks page, and Medulla installs it into whichever coding CLI it
//! spawns. Without this, the same policy — "checkpoint after every edit",
//! "refuse writes outside the worktree", "log every prompt" — has to be written
//! once per harness, in three different config languages, and drifts the moment
//! one of them is edited.
//!
//! Medulla installs hooks of its own too, on by default: see [`builtin`] for the
//! reporting set that makes a harness on a pty legible, and [`HookReport`] for
//! what one of them puts on the wire.
//!
//! # Vocabulary
//!
//! The canonical events ([`HookEvent`]) are not a Medulla invention. Claude Code
//! 2.1.221 and Codex 0.146 independently converged on the same event names, the
//! same `matcher` + `hooks[]` grouping, and the same
//! `{"type":"command","command":…}` handler — so the shared document is built
//! once in [`native`] and only its *delivery* is per-harness. `Notification` is
//! the single divergence: Claude Code raises it, Codex does not.
//!
//! # Delivery
//!
//! Nothing is persisted. Like [`crate::attribution`], hooks are injected per
//! spawn, so an operator's own `claude` and `codex` sessions keep exactly the
//! hooks they configured and only Medulla-launched harnesses carry Medulla's.
//!
//! - **Claude Code** takes the document inside `--settings`, which layers over
//!   the user's `settings.json`. Medulla already spends that flag on attribution,
//!   so the two share one JSON object — see [`launch_args`], which is why callers
//!   must not add `--settings` separately.
//! - **Codex** takes it as a `-c hooks=<inline TOML>` config override. Config
//!   layers are additive in Codex, so this adds to the operator's own
//!   `hooks.json` rather than replacing it. Codex also records trust against a
//!   hook's hash and *silently skips* hooks it has not been told to run, so
//!   delivery alone is not enough: [`codex::trust`] enables this spawn's own
//!   entries in `$CODEX_HOME/config.toml` as part of building the injection.
//!   That is scoped to the hooks Medulla injected and no others, which is
//!   exactly what `--dangerously-bypass-hook-trust` cannot do — it is
//!   invocation-wide and would also authorize hooks the checked-out repository
//!   ships in its own `.codex/hooks.json`, so Medulla still does **not** pass
//!   it. A hook Codex has never seen costs one launch before it starts firing;
//!   that, and any failure to write the trust store, are reported through
//!   [`HookInjection::warnings`] rather than left to be discovered.
//! - **OpenCode** exposes hooks as a scripted plugin API rather than a
//!   declarative command hook, and is not adapted yet; its hooks are reported as
//!   [`DroppedHook`]s rather than silently ignored.
//! - **OpenHuman** runs in-process and has no external harness to configure; its
//!   `PreToolUse`, `PostToolUse`, and `Stop` hooks are instead routed to the
//!   embedded core's embedder hooks at boot (see [`crate::core_host::hooks`]),
//!   so `hook_injection` here carries no argv for it.
//!
//! Both delivery paths were verified end-to-end against the versions named
//! above: a `SessionStart` hook injected this way fires on Claude Code, and on
//! Codex fires alongside the operator's own once trusted.
//!
//! All of that assumes Medulla spawns the harness CLI itself. Under ACP
//! dispatch it spawns an ACP *server* which spawns the harness, so there is no
//! argv to add to and these two paths deliver nothing at all — see [`acp`],
//! which carries the same document over that transport's own channels.

use crate::protocol::HarnessProvider;

pub mod acp;
pub mod builtin;
mod claude;
mod codex;
mod grant;
mod native;
mod report;
mod types;

#[cfg(test)]
mod tests;

pub use acp::{delivery as acp_delivery, AcpDelivery};
pub use grant::{seed_hook_grant, HookGrantGuard};
pub use report::{HookEventLog, HookReport};
pub use types::{
    DroppedHook, HookEvent, HookHandler, HookInjection, HookSpec, HooksConfig, LaunchPolicy,
};

/// Build the per-spawn injection that installs `hooks` into `provider`.
///
/// Returns an empty injection when no declared hook applies. Hooks the harness
/// cannot run are reported in [`HookInjection::dropped`] rather than dropped
/// silently.
///
/// Callers that also apply commit attribution to Claude Code must use
/// [`launch_args`] instead of combining this with
/// [`crate::attribution::attribution_args`]: both want `--settings`, and passing
/// it twice loses one of them.
///
/// This function is pure, and has to stay that way: the Hooks page calls it on
/// every frame to describe what each harness would carry. Approving Medulla's
/// hooks in Codex's trust store — the one part of installation that writes
/// anything — happens in [`launch_args`], which only a real spawn calls.
pub fn hook_injection(provider: HarnessProvider, hooks: &HooksConfig) -> HookInjection {
    let mut injection = HookInjection {
        dropped: dropped_for(provider, hooks),
        ..HookInjection::default()
    };
    let applicable = hooks.for_provider(provider);
    if applicable.is_empty() {
        return injection;
    }
    let document = native::hook_document(&applicable);
    match provider {
        HarnessProvider::Claude => injection.args = claude::settings_args(&document),
        HarnessProvider::Codex => injection.args = codex::config_args(&document),
        HarnessProvider::Opencode | HarnessProvider::Openhuman | HarnessProvider::Shell => {}
    }
    injection
}

/// The events `provider` will actually be installing from `hooks`.
fn installed_events(provider: HarnessProvider, hooks: &HooksConfig) -> Vec<HookEvent> {
    hooks
        .for_provider(provider)
        .iter()
        .map(|hook| hook.event)
        .collect()
}

/// The complete argv prefix for a Medulla-launched `provider`: commit
/// attribution and the operator's hooks, merged.
///
/// This exists because Claude Code carries both through the same `--settings`
/// flag. Passing the flag once per feature would silently drop whichever came
/// first, so the two settings fragments are merged into a single object here.
/// Every other provider simply concatenates.
///
/// `attribution` is the resolved `attribution.commit` config value; `env` is the
/// environment the child will be spawned with.
///
/// This is the spawn path, and the one place installation *writes* anything: a
/// Codex launch approves its own injected hooks in the trust store under `env`'s
/// `CODEX_HOME` (see [`codex::trust`]) before handing them over, because Codex
/// silently skips a hook it has not been told to run. Nothing else is touched
/// there, and no other provider is written to at all. Callers that only want to
/// *describe* an injection want [`hook_injection`], which stays pure.
pub fn launch_args(
    provider: HarnessProvider,
    attribution: bool,
    hooks: &HooksConfig,
    env: &std::collections::HashMap<String, String>,
) -> (Vec<String>, Vec<String>) {
    let mut injection = hook_injection(provider, hooks);
    if provider == HarnessProvider::Codex && !injection.args.is_empty() {
        injection.warnings.extend(codex::trust_injected(
            &installed_events(provider, hooks),
            env,
        ));
    }
    if provider != HarnessProvider::Claude {
        let mut args = crate::attribution::attribution_args(provider, attribution);
        args.extend(injection.args.iter().cloned());
        return (args, injection.notes());
    }

    let mut settings = serde_json::Map::new();
    if attribution {
        settings.insert(
            "attribution".into(),
            serde_json::json!({ "commit": crate::attribution::attribution_trailer() }),
        );
    }
    let applicable = hooks.for_provider(provider);
    if !applicable.is_empty() {
        settings.insert("hooks".into(), native::hook_document(&applicable));
    }
    let args = if settings.is_empty() {
        Vec::new()
    } else {
        vec![
            "--settings".to_string(),
            serde_json::Value::Object(settings).to_string(),
        ]
    };
    (args, injection.notes())
}

/// The hooks `provider` cannot run, with the reason for each.
fn dropped_for(provider: HarnessProvider, hooks: &HooksConfig) -> Vec<DroppedHook> {
    hooks
        .hooks
        .iter()
        .filter(|hook| {
            let selected = hook.harnesses.is_empty() || hook.harnesses.contains(&provider);
            selected && !hook.event.supported_by(provider)
        })
        .map(|hook| DroppedHook {
            event: hook.event,
            provider,
            reason: unsupported_reason(provider, hook.event),
        })
        .collect()
}

/// Why `provider` cannot run `event`, phrased for an operator reading a log.
fn unsupported_reason(provider: HarnessProvider, event: HookEvent) -> String {
    match provider {
        HarnessProvider::Opencode => {
            "OpenCode exposes hooks through its scripted plugin API, which Medulla does not \
             translate to yet"
                .to_string()
        }
        HarnessProvider::Openhuman => {
            "OpenHuman's embedded runner currently raises PreToolUse, PostToolUse, and Stop"
                .to_string()
        }
        HarnessProvider::Shell => "a shell raises no hook events".to_string(),
        HarnessProvider::Claude | HarnessProvider::Codex => {
            format!(
                "{} does not raise the {} event",
                provider.display_name(),
                event.as_str()
            )
        }
    }
}
