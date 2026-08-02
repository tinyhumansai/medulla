//! Detecting whether a harness is waiting on the operator.

use medulla::tinyplace::HarnessProvider;

use super::super::dialog::blocking_dialog_for;
use super::AttentionKind;

/// Phrases that mean the harness is *working*, not waiting.
///
/// Claude and Codex both footer their running turn with "esc to interrupt". A
/// screen carrying one of these is busy, which is what vetoes the vaguest cues:
/// a bell rung as a tool finishes must not leave a working harness flagged as
/// blocked for the rest of its turn.
const WORKING: &[&str] = &["esctointerrupt", "esctocancel", "ctrlctostop"];

/// What each harness puts on screen when it is asking the operator something.
///
/// Read as: if any marker in the group is on screen, the harness wants
/// `what`. Stored squashed (lowercase, alphanumerics only) so they survive the
/// layout of a full-screen TUI — see [`squash`].
///
/// Claude's and Codex's entries are taken from their installed CLIs. OpenCode's
/// permission menu is matched structurally below because its individual labels
/// are too generic to be safe markers on their own.
const MARKERS: &[(HarnessProvider, &[&str], AttentionKind, &str)] = &[
    (
        HarnessProvider::Claude,
        // These are option labels unique to Claude's permission menu. Generic
        // question wording is deliberately absent: it can appear in ordinary
        // conversation while Claude is still working.
        &["noandtellclaudewhattodo", "yesanddontaskagain"],
        AttentionKind::Approval,
        "claude is asking permission",
    ),
    (
        HarnessProvider::Claude,
        // Plan mode's exit prompt. "keep planning" is its third option and is
        // unique to it, so it is named separately from the tool prompts above:
        // an operator reading the rail wants to know a plan is ready, which is
        // a different thing to answer than a shell command.
        &["keepplanning"],
        AttentionKind::Approval,
        "claude finished planning and wants a decision",
    ),
    (
        HarnessProvider::Codex,
        &["andtellcodexwhattodo"],
        AttentionKind::Approval,
        "codex is asking permission",
    ),
];

/// Whether OpenCode drew its permission action menu.
///
/// Each label alone is ordinary prose ("always allow retries", "allow once").
/// Their combination is the recognizable menu context and avoids flagging
/// retained conversation even while the working footer is visible.
fn has_opencode_permission_menu(squashed: &str) -> bool {
    squashed.contains("allowonce") && squashed.contains("alwaysallow")
}

/// Carets a harness rests on the option it would take if you pressed Return.
///
/// The three CLIs disagree on the glyph and agree on everything else, so the
/// caret is what the structural fallback keys on rather than any wording.
// ASCII `>` is deliberately absent: every harness also uses it for the input
// composer, where a perfectly ordinary draft can begin with `1. `.
const CARETS: &[char] = &['❯', '›', '▸', '»'];

/// Strip to lowercase alphanumerics so a match does not depend on layout.
///
/// A full-screen TUI positions text with cursor moves and box-drawing, so the
/// spaces, borders, and punctuation between two words are an accident of the
/// pane's width. What survives that is the letters.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether the screen says the harness is mid-turn.
///
/// Public because the manager needs it for the bell: a bell that arrives while
/// the harness is plainly working is a progress chime, not a request.
pub fn is_working(screen: &str) -> bool {
    let squashed = squash(screen);
    WORKING.iter().any(|marker| squashed.contains(marker))
}

/// Whether a line is a *numbered* option with the cursor resting on it.
///
/// `❯ 1. Yes`, `› 2. No`, `> 3) Skip`. Two halves, and both are load-bearing:
///
/// - the **caret**, because it is drawn on the option Return would take, which
///   only happens while something is being asked;
/// - the **number**, because a caret alone is also how every one of these
///   harnesses draws its *composer* (`> Try "fix the failing test"`). Keying on
///   the caret alone flagged an idle harness as blocked — the one false positive
///   that would have made the whole feature noise, since a composer is what a
///   harness shows for most of its life.
///
/// A menu whose options are not numbered therefore does not match here; it is
/// left to the per-harness markers, which is the right trade. A missed blink is
/// cheaper than a rail that blinks at nothing.
pub(super) fn is_selected_option(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '│', '┃', '|']).trim_start();
    let mut chars = trimmed.chars();
    if !chars.next().is_some_and(|first| CARETS.contains(&first)) {
        return false;
    }
    let rest = chars.as_str().trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return false;
    }
    let after = &rest[digits.len()..];
    // A label after the separator, so a stray `> 1` in output is not a menu.
    matches!(after.chars().next(), Some('.') | Some(')'))
        && after[1..].trim_start().chars().next().is_some()
}

/// Whether a selected option belongs to the live bottom-of-screen prompt.
///
/// Completed menus remain in terminal scrollback after a CLI returns to its
/// composer. Bound the search to the active tail and reject any candidate with
/// a composer below it, while leaving room for the menu's remaining choices and
/// closing border.
fn has_active_selected_option(screen: &str) -> bool {
    let lines: Vec<&str> = screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail_start = lines.len().saturating_sub(6);
    lines
        .iter()
        .enumerate()
        .skip(tail_start)
        .any(|(index, line)| {
            is_selected_option(line) && !lines[index + 1..].iter().any(|line| is_composer(line))
        })
}

/// Whether `line` is an idle input composer rather than a numbered option.
fn is_composer(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '│', '┃', '|']).trim_start();
    trimmed
        .chars()
        .next()
        .is_some_and(|first| first == '>' || CARETS.contains(&first))
        && !is_selected_option(line)
}

/// Whether the live screen tail has the controls of a startup modal.
///
/// Dialog phrases can remain in conversation history. The numbered selection
/// and confirmation footer distinguish a modal the operator can act on from
/// retained prose, while the composer check rejects an already-dismissed modal.
fn has_active_dialog_context(screen: &str) -> bool {
    let lines: Vec<&str> = screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.last().is_some_and(|line| is_composer(line)) {
        return false;
    }
    let tail_start = lines.len().saturating_sub(8);
    let tail = &lines[tail_start..];
    let footer = squash(&tail.join("\n"));
    let has_confirmation =
        footer.contains("entertoconfirm") || footer.contains("pressentertocontinue");
    has_confirmation && tail.iter().any(|line| is_dialog_selected_option(line))
}

/// Whether `line` is the selected numbered row of a startup modal.
///
/// Startup dialogs use ASCII `>` as well as the harness-specific carets. This
/// stricter helper is safe because callers also require the modal footer.
fn is_dialog_selected_option(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '│', '┃', '|']).trim_start();
    let mut chars = trimmed.chars();
    if !chars
        .next()
        .is_some_and(|first| first == '>' || CARETS.contains(&first))
    {
        return false;
    }
    let rest = chars.as_str().trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return false;
    }
    let after = &rest[digits.len()..];
    matches!(after.chars().next(), Some('.') | Some(')'))
        && after[1..].trim_start().chars().next().is_some()
}

/// Whether the screen carries a bare yes/no confirmation.
///
/// Matched on punctuation, so it is checked against the raw text rather than
/// the squashed form — `(y/n)` squashes to `yn`, which appears in ordinary
/// prose.
fn has_yes_no(screen: &str) -> bool {
    // A terminal retains old output above its live prompt. Only the bottommost
    // non-empty line can be the active bare confirmation; searching the whole
    // viewport makes a completed `(y/n)` exchange blink forever while the
    // composer below it is idle.
    let Some(line) = screen.lines().rev().find(|line| !line.trim().is_empty()) else {
        return false;
    };
    let lower = line.to_lowercase();
    ["(y/n)", "[y/n]", "(y/n/a)", "[y/n/a]", "(yes/no)"]
        .iter()
        .any(|marker| lower.contains(marker))
}

/// What `screen` says this harness is waiting for, if anything.
///
/// Returns the cue *and* its wording; the caller stamps it with a time and
/// decides whether it displaces what the row already carries. `None` means the
/// screen shows no question we can recognise — which is the common case, and
/// deliberately not the same as "the harness is busy".
pub fn detect(provider: HarnessProvider, screen: &str) -> Option<(AttentionKind, String)> {
    // A recognised startup dialog outranks everything: it is the one case where
    // the harness will not take work at all, and it already has wording written
    // for an operator.
    let dialog = (!is_working(screen) && has_active_dialog_context(screen))
        .then(|| blocking_dialog_for(provider, screen))
        .flatten();
    if let Some(dialog) = dialog {
        return Some((AttentionKind::Dialog, dialog.what.to_string()));
    }

    let squashed = squash(screen);
    if let Some((_, _, kind, what)) = MARKERS
        .iter()
        .filter(|(candidate, ..)| *candidate == provider)
        .find(|(_, markers, ..)| markers.iter().any(|marker| squashed.contains(marker)))
    {
        return Some((*kind, (*what).to_string()));
    }
    if provider == HarnessProvider::Opencode && has_opencode_permission_menu(&squashed) {
        return Some((
            AttentionKind::Approval,
            "opencode is asking permission".to_string(),
        ));
    }

    // Unrecognised wording, recognisable shape. Vetoed while the harness is
    // plainly mid-turn, because a caret can survive on a screen the harness is
    // still painting over.
    if is_working(screen) {
        return None;
    }
    if has_active_selected_option(screen) {
        return Some((
            AttentionKind::Choice,
            format!("{} is waiting on a choice", provider.as_str()),
        ));
    }
    if has_yes_no(screen) {
        return Some((
            AttentionKind::Choice,
            format!("{} wants a yes or no", provider.as_str()),
        ));
    }
    None
}

/// The cue for a harness that rang the terminal bell.
///
/// Kept here rather than in the manager so every piece of attention vocabulary
/// is written in one place.
pub fn bell_cue(provider: HarnessProvider) -> (AttentionKind, String) {
    (
        AttentionKind::Bell,
        format!("{} rang the bell", provider.as_str()),
    )
}
