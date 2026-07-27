//! Pairing hand-off: getting a worker's address from the machine it runs on to
//! the orchestrator that will delegate to it.
//!
//! Adding a host is a one-string problem. The orchestrator opens the channel and
//! the daemon auto-accepts the contact, so nothing has to travel *to* the
//! worker — only the worker's tiny.place address has to travel *back*. The
//! awkward part is that the worker is usually a box you are on over SSH, where
//! selecting a base58 string out of scrollback is exactly the fiddly,
//! error-prone step that makes people give up.
//!
//! Two things fix that, and this module owns both:
//!
//! - [`clipboard_handoff`] writes the address to the *operator's* clipboard
//!   through the terminal itself (OSC 52). The escape is interpreted by the
//!   terminal emulator on the near side of the SSH connection, so the address
//!   lands on the laptop you are typing at, not on the remote host. This is the
//!   one clipboard mechanism that crosses an SSH boundary, which is why the
//!   spawn-a-binary path ([`crate::clipboard::copy_to_clipboard`]) is
//!   deliberately *not* used here — `pbcopy` on a remote Mac would copy to a
//!   clipboard nobody is looking at.
//! - [`pairing_banner`] prints the address on a line of its own, framed, with
//!   the exact next step. Even when the clipboard hand-off is refused (a
//!   terminal with OSC 52 disabled), a double-click-selectable line beats a
//!   fragment buried in a log message.
//!
//! Both are pure functions over their inputs so the wording and the escape are
//! testable without a terminal; [`super::entry`] does the writing.

use crate::clipboard::osc52;

/// The single line to paste into a shell on the machine being added.
///
/// One paste, in the easy direction: it is copied on the orchestrator — a local
/// terminal, where copying is trivial — and pasted into the remote session.
/// Installing is guarded rather than unconditional so pasting it onto a machine
/// that already has `medulla` starts the worker instead of reinstalling it, and
/// so the whole thing stays one line rather than a decision the operator has to
/// make before running anything.
///
/// # Trust
///
/// The binary this installs *is* verified: `install.sh` resolves the release
/// manifest, downloads the platform archive, and refuses to install on a SHA-256
/// mismatch. What is not pinned is `install.sh` itself, which is read from the
/// default branch — so the trust root is that fetch over TLS from GitHub, and a
/// compromised branch would be executed. Closing that would mean publishing the
/// installer as a signed release asset and pinning this URL to it; until then
/// this is the same exposure as the documented `curl … | sh` in the README, and
/// the guard means it is not even reached on a machine that already has the
/// binary.
pub const REMOTE_JOIN_COMMAND: &str = "command -v medulla >/dev/null 2>&1 || \
     curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh; \
     medulla daemon";

/// The OSC 52 escape that hands `address` to the operator's terminal clipboard.
///
/// Returns `None` when `stdout_is_terminal` is false. A daemon under systemd or
/// in a pipeline has no terminal to talk to, and emitting the escape anyway
/// would put a control sequence into a log file.
pub fn clipboard_handoff(address: &str, stdout_is_terminal: bool) -> Option<String> {
    let address = address.trim();
    if address.is_empty() || !stdout_is_terminal {
        return None;
    }
    Some(osc52(address))
}

/// The framed pairing block printed once the daemon is onboarded.
///
/// `handle` is the `@handle` this worker registered, when it has one — with a
/// handle the operator can type `@name` on the orchestrator and skip the copy
/// entirely, so it is worth naming right where the address is.
///
/// `copied` reflects whether [`clipboard_handoff`] fired, so the block never
/// claims a clipboard it did not reach.
pub fn pairing_banner(address: &str, handle: Option<&str>, copied: bool) -> String {
    let rule = "─".repeat(64);
    let mut out = String::new();
    out.push_str(&rule);
    out.push_str("\n  This worker's address:\n\n    ");
    out.push_str(address.trim());
    out.push_str("\n\n");
    if let Some(handle) = handle.map(str::trim).filter(|h| !h.is_empty()) {
        let handle = handle.strip_prefix('@').unwrap_or(handle);
        out.push_str(&format!(
            "  Registered as @{handle} — on the orchestrator you can type that\n  \
             instead of the address above.\n\n"
        ));
    }
    if copied {
        out.push_str("  Copied to your clipboard (via the terminal, so it works over SSH).\n\n");
    }
    out.push_str(
        "  On the orchestrator: Routing › Add Host › press a, then paste.\n  \
         Leave this daemon running; it accepts the connection automatically.\n",
    );
    out.push_str(&rule);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests;
