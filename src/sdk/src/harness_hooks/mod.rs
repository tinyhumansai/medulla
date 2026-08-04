//! Standardized lifecycle hooks for Medulla-launched harnesses.
//!
//! An operator declares a hook once, in Medulla's own config, and Medulla
//! installs it into whichever coding CLI it spawns. Without this, the same
//! policy — "checkpoint after every edit", "refuse writes outside the worktree",
//! "log every prompt" — has to be written once per harness, in three different
//! config languages, and drifts the moment one of them is edited.
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
//!   `hooks.json` rather than replacing it. Codex records trust against a hook's
//!   hash and *silently skips* hooks it has not seen before, and a per-spawn
//!   injection is by construction never in that store — so a Medulla hook does
//!   nothing on Codex until the operator trusts it once (`/hooks`). Medulla does
//!   **not** pass `--dangerously-bypass-hook-trust`: it is invocation-wide and
//!   would also authorize hooks the checked-out repository ships in its own
//!   `.codex/hooks.json`. Requiring one explicit trust is the safe direction to
//!   fail, and the requirement is reported through
//!   [`HookInjection::warnings`] rather than left to be discovered.
//! - **OpenCode** exposes hooks as a scripted plugin API rather than a
//!   declarative command hook, and is not adapted yet; its hooks are reported as
//!   [`DroppedHook`]s rather than silently ignored.
//! - **OpenHuman** runs in-process and has no external harness to configure.
//!
//! Both delivery paths were verified end-to-end against the versions named
//! above: a `SessionStart` hook injected this way fires on Claude Code, and on
//! Codex fires alongside the operator's own once trusted.

use crate::protocol::HarnessProvider;

mod claude;
mod codex;
mod native;
mod types;

#[cfg(test)]
mod tests;

pub use types::{DroppedHook, HookEvent, HookHandler, HookInjection, HookSpec, HooksConfig};

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
        HarnessProvider::Codex => {
            let (args, warning) = codex::config_args(&document);
            injection.args = args;
            injection.warnings.push(warning);
        }
        HarnessProvider::Opencode | HarnessProvider::Openhuman => {}
    }
    injection
}

/// The complete argv prefix for a Medulla-launched `provider`: commit
/// attribution and the operator's hooks, merged.
///
/// This exists because Claude Code carries both through the same `--settings`
/// flag. Passing the flag once per feature would silently drop whichever came
/// first, so the two settings fragments are merged into a single object here.
/// Every other provider simply concatenates.
///
/// `attribution` is the resolved `attribution.commit` config value.
pub fn launch_args(
    provider: HarnessProvider,
    attribution: bool,
    hooks: &HooksConfig,
) -> (Vec<String>, Vec<String>) {
    let injection = hook_injection(provider, hooks);
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
            "OpenHuman runs in-process and has no external harness config to install hooks into"
                .to_string()
        }
        HarnessProvider::Claude | HarnessProvider::Codex => {
            format!(
                "{} does not raise the {} event",
                provider.display_name(),
                event.as_str()
            )
        }
    }
}
