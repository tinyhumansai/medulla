//! The credential a spawned harness's built-in hook commands need to reach
//! `hook.report`.
//!
//! [`builtin`](super::builtin) installs the same `medulla hook <Event>` command
//! into every spawn door, but only the PTY door
//! (`worker::pty::launch::attach_mcp` in the `medulla-tui` crate) used to seed
//! the environment variables that command needs to find a control plane to
//! report to. The headless one-shot executor
//! ([`crate::daemon::providers::execute`]), the interactive session
//! ([`crate::sessions::interactive`]), and the `medulla <cli>` wrapper
//! ([`crate::wrapper`]) all installed the hooks and none of them seeded the
//! grant — so on those doors every built-in spawned, found nothing to report
//! to, and exited, at the cost of one wasted process per hook firing (and
//! `PostToolUse` fires once per tool call). [`seed_hook_grant`] is the one
//! seam all four doors now share.

use std::collections::HashMap;

/// Mint (when this process serves a control plane) and seed `env` with the
/// hook-only grant a spawned harness's `medulla hook <Event>` commands need
/// to reach `hook.report` — see
/// `control_socket::grants::Grant::hook_only`'s own docs for why this
/// credential is safe to hand to every subprocess a harness starts, unlike
/// the fleet grant.
///
/// Returns a guard that revokes the grant when it drops, so a spawn that
/// outlives its own function scope — an interactive session held across
/// several turns, not just a single `async fn` — still gives its grant back
/// exactly when the session that redeemed it ends, rather than leaving a live
/// token in the registry for the rest of this process's life.
///
/// `session` must be unique per spawn: two spawns sharing one key would share
/// one grant, and revoking either would revoke both — see
/// `mcp::attach::local_hook_grant`'s own docs, which this calls.
///
/// A no-op guard — nothing minted, nothing to revoke — exactly when
/// `mcp::attach::local_hook_grant` itself returns `None`: no control plane
/// bound (a remote worker's daemon, a `medulla workflow` invocation), which is
/// also every build without the `workflows` feature, since the control
/// plane's grant registry lives behind it.
#[cfg(feature = "workflows")]
pub fn seed_hook_grant(
    session: impl Into<String>,
    env: &mut HashMap<String, String>,
) -> HookGrantGuard {
    let session = session.into();
    match crate::mcp::local_hook_grant(&session) {
        Some((socket, token)) => {
            env.insert(
                crate::control_socket::HOOK_SOCKET_ENV.to_string(),
                socket.to_string_lossy().into_owned(),
            );
            env.insert(crate::control_socket::HOOK_GRANT_ENV.to_string(), token);
            HookGrantGuard(Some(session))
        }
        None => HookGrantGuard(None),
    }
}

/// [`seed_hook_grant`] without the `workflows` feature: the control plane and
/// its grant registry live entirely behind it, so there is nothing here to
/// mint or seed. Kept as a same-shaped no-op rather than `#[cfg]`-ing every
/// call site, matching how `worker::pty::launch::attach_mcp` (in the
/// `medulla-tui` crate) already handles the same split.
#[cfg(not(feature = "workflows"))]
pub fn seed_hook_grant(
    session: impl Into<String>,
    env: &mut HashMap<String, String>,
) -> HookGrantGuard {
    let _ = (session, env);
    HookGrantGuard
}

/// Revokes [`seed_hook_grant`]'s grant, if one was minted, when dropped.
#[cfg(feature = "workflows")]
pub struct HookGrantGuard(Option<String>);

#[cfg(feature = "workflows")]
impl Drop for HookGrantGuard {
    fn drop(&mut self) {
        if let Some(session) = self.0.take() {
            crate::mcp::revoke_session(&session);
        }
    }
}

/// [`HookGrantGuard`] without the `workflows` feature: nothing was ever
/// minted, so there is nothing to revoke on drop either.
#[cfg(not(feature = "workflows"))]
pub struct HookGrantGuard;

#[cfg(test)]
#[path = "grant_tests.rs"]
mod tests;
