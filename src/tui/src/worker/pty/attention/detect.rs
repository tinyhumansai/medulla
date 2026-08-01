//! Detecting whether a harness is waiting on the operator.

use medulla::tinyplace::HarnessProvider;

use super::super::dialog::blocking_dialog;
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
/// are its documented permission wording and are the least certain of the three;
/// they are additive, so if OpenCode words a prompt differently the structural
/// fallback below still catches the menu it drew.
const MARKERS: &[(HarnessProvider, &[&str], AttentionKind, &str)] = &[
    (
        HarnessProvider::Claude,
        // "3. No, and tell Claude what to do differently" is on every one of
        // claude's permission prompts and on nothing else, which makes it the
        // load-bearing marker here; the rest catch the prompt by its question.
        &[
            "noandtellclaudewhattodo",
            "doyouwanttoproceed",
            "doyouwanttomakethisedit",
            "doyouwanttocreate",
            "yesanddontaskagain",
        ],
        AttentionKind::Approval,
        "claude is asking permission",
    ),
    (
        HarnessProvider::Claude,
        // Plan mode's exit prompt. "keep planning" is its third option and is
        // unique to it, so it is named separately from the tool prompts above:
        // an operator reading the rail wants to know a plan is ready, which is
        // a different thing to answer than a shell command.
        &["keepplanning", "wouldyouliketoproceed"],
        AttentionKind::Approval,
        "claude finished planning and wants a decision",
    ),
    (
        HarnessProvider::Codex,
        &["andtellcodexwhattodo", "allowcodexto", "codexwantsto"],
        AttentionKind::Approval,
        "codex is asking permission",
    ),
    (
        HarnessProvider::Opencode,
        &[
            "opencodewantsto",
            "alwaysallow",
            "allowonce",
            "rejectrequest",
            "permissionrequired",
        ],
        AttentionKind::Approval,
        "opencode is asking permission",
    ),
];

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

/// Whether the screen carries a bare yes/no confirmation.
///
/// Matched on punctuation, so it is checked against the raw text rather than
/// the squashed form — `(y/n)` squashes to `yn`, which appears in ordinary
/// prose.
fn has_yes_no(screen: &str) -> bool {
    let lower = screen.to_lowercase();
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
    if let Some(dialog) = blocking_dialog(screen) {
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

    // Unrecognised wording, recognisable shape. Vetoed while the harness is
    // plainly mid-turn, because a caret can survive on a screen the harness is
    // still painting over.
    if is_working(screen) {
        return None;
    }
    if screen.lines().any(is_selected_option) {
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
