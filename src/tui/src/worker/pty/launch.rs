//! Launching a harness CLI in its **interactive** mode on a pseudo-terminal.
//!
//! This is the difference that makes the worker TUI possible. The daemon's
//! headless path runs `claude -p --output-format stream-json`, which emits JSON
//! and paints nothing; there is no screen to show. Here the CLI is started the
//! way a human starts it — no `-p`, no `--output-format` — so it renders its own
//! full-screen interface, and the PTY gives us that interface as a byte stream
//! to parse.
//!
//! A tty is not optional: Codex refuses to start with `stdin is not a terminal`,
//! and both harnesses fall back to dumb line mode without one.

use medulla::tinyplace::HarnessProvider;

/// The argv for an interactive (screen-painting) run of `provider`.
///
/// Deliberately minimal. Every flag the headless path adds — `-p`,
/// `--output-format`, `--verbose` — exists to *suppress* the interface we are
/// here to render, so none of them belong on this argv. A prompt is not passed
/// either: work arrives by typing into the PTY, exactly as a human would.
pub fn interactive_args(
    provider: HarnessProvider,
    session_id: Option<&str>,
    skip_permissions: bool,
    extra: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if provider == HarnessProvider::Opencode {
        // `opencode` needs its TUI subcommand; `claude` and `codex` paint by
        // default when handed a tty.
        args.push("tui".to_string());
    }
    if skip_permissions {
        args.extend(bypass_flag(provider).iter().map(|f| f.to_string()));
    }
    // Only claude accepts a preset id. Handing one over turns transcript
    // attribution from a guess ("newest file in this directory") into a fact
    // ("the file named after this session"), which is what makes two sessions in
    // one repo safe.
    if let (HarnessProvider::Claude, Some(id)) = (provider, session_id) {
        args.push("--session-id".to_string());
        args.push(id.to_string());
    }
    args.extend(extra.iter().cloned());
    args
}

/// The flag that stops a provider asking permission before each tool call.
///
/// A watched session is still an *unattended* one: the operator is looking at a
/// pane, not sitting in it answering prompts, and a peer's task that stops on
/// "allow this command?" has silently hung until its timeout. The harnesses
/// spell it differently, and neither name is one to guess at — both are taken
/// from the installed CLIs' own `--help`.
///
/// `opencode` gets nothing: it is refused for delegated work anyway, since it
/// writes no transcript a turn's completion could be read from.
pub fn bypass_flag(provider: HarnessProvider) -> &'static [&'static str] {
    match provider {
        HarnessProvider::Claude => &["--dangerously-skip-permissions"],
        HarnessProvider::Codex => &["--dangerously-bypass-approvals-and-sandbox"],
        HarnessProvider::Opencode => &[],
    }
}

/// Whether a provider accepts a preset session id.
///
/// Codex does not: `codex resume <id>` takes an *existing* id, and there is no
/// flag to choose one for a fresh session. Its rollout records its own id on
/// line one instead, so attribution reads rather than dictates.
pub fn accepts_preset_session_id(provider: HarnessProvider) -> bool {
    provider == HarnessProvider::Claude
}

/// Mint a session id for a provider that accepts one.
pub fn mint_session_id(provider: HarnessProvider) -> Option<String> {
    accepts_preset_session_id(provider).then(|| uuid::Uuid::new_v4().to_string())
}

/// Whether a provider paints a full-screen interface worth embedding.
///
/// All three do when given a tty; this exists so the UI can say something
/// truthful if that ever stops being true for one of them.
pub fn paints_a_screen(provider: HarnessProvider) -> bool {
    matches!(
        provider,
        HarnessProvider::Claude | HarnessProvider::Codex | HarnessProvider::Opencode
    )
}

/// The keystrokes that submit a line of injected text to a harness.
///
/// A carriage return, not a newline: the child's line discipline is in raw mode
/// and its TUI reads `\r` as Enter. Sending `\n` types a literal newline into
/// the composer of both Claude Code and Codex instead of submitting.
pub fn submit_sequence() -> &'static [u8] {
    b"\r"
}

/// Bracketed-paste wrapping for injected text.
///
/// Peer prompts can be long and contain newlines. Typed raw, every embedded
/// newline submits a partial prompt. Bracketing tells the harness "this is one
/// paste", which both Claude Code and Codex honour by inserting it as a single
/// block.
pub fn bracket_paste(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interactive_argv_suppresses_nothing() {
        // The headless flags exist to hide the interface we are rendering.
        for provider in [HarnessProvider::Claude, HarnessProvider::Codex] {
            let args = interactive_args(provider, None, false, &[]);
            assert!(args.is_empty(), "{provider:?} paints by default: {args:?}");
        }
    }

    #[test]
    fn each_harness_gets_its_own_permission_bypass_flag() {
        // The two spell it differently and neither name is guessable; both are
        // taken from the installed CLIs' `--help`. Getting one wrong means the
        // harness exits on an unknown argument, so pin the exact strings.
        assert_eq!(
            interactive_args(HarnessProvider::Claude, None, true, &[]),
            vec!["--dangerously-skip-permissions"]
        );
        assert_eq!(
            interactive_args(HarnessProvider::Codex, None, true, &[]),
            vec!["--dangerously-bypass-approvals-and-sandbox"]
        );
    }

    #[test]
    fn the_bypass_is_off_unless_asked_for() {
        // Running someone's harness without permission checks is not something
        // to arrive at by default in this function; the caller decides.
        for provider in [HarnessProvider::Claude, HarnessProvider::Codex] {
            let args = interactive_args(provider, Some("s1"), false, &[]);
            assert!(
                !args.iter().any(|a| a.starts_with("--dangerous")),
                "{provider:?}: {args:?}"
            );
        }
    }

    #[test]
    fn the_bypass_precedes_the_session_id_and_extras() {
        // claude parses flags in order and `--session-id` takes a value; a flag
        // landing between them would consume it.
        let args = interactive_args(
            HarnessProvider::Claude,
            Some("abc-123"),
            true,
            &["--model".into(), "x".into()],
        );
        assert_eq!(
            args,
            vec![
                "--dangerously-skip-permissions",
                "--session-id",
                "abc-123",
                "--model",
                "x"
            ]
        );
    }

    #[test]
    fn opencode_needs_its_tui_subcommand() {
        assert_eq!(
            interactive_args(HarnessProvider::Opencode, None, false, &[]),
            vec!["tui"]
        );
    }

    #[test]
    fn extra_args_follow_the_base() {
        let args = interactive_args(
            HarnessProvider::Opencode,
            None,
            false,
            &["--model".into(), "x".into()],
        );
        assert_eq!(args, vec!["tui", "--model", "x"]);
    }

    #[test]
    fn submit_is_a_carriage_return_not_a_newline() {
        // `\n` types a literal newline into the composer instead of submitting.
        assert_eq!(submit_sequence(), b"\r");
    }

    #[test]
    fn injected_text_is_bracketed_as_one_paste() {
        let out = bracket_paste("line one\nline two");
        assert!(out.starts_with(b"\x1b[200~"));
        assert!(out.ends_with(b"\x1b[201~"));
        // The embedded newline must survive — bracketing is what stops it
        // submitting a partial prompt.
        assert!(String::from_utf8_lossy(&out).contains("line one\nline two"));
    }
}
