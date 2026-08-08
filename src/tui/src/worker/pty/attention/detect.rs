//! Detecting whether a harness is waiting on the operator.

use medulla::protocol::HarnessProvider;

use super::super::dialog::blocking_dialog_for;
use super::AttentionKind;

/// Phrases that mean the harness is *working*, not waiting.
///
/// Codex still footers its running turn with "esc to interrupt", and so do the
/// older Claude builds. A screen carrying one of these is busy, which is what
/// vetoes the vaguest cues: a bell rung as a tool finishes must not leave a
/// working harness flagged as blocked for the rest of its turn.
///
/// The last entry is Claude's composer placeholder while a turn is in flight —
/// it replaces the ordinary hint text for exactly as long as the harness is
/// busy, which makes it the one *phrase* current Claude reliably offers.
const WORKING: &[&str] = &[
    "esctointerrupt",
    "esctocancel",
    "ctrlctostop",
    "pressuptoeditqueuedmessages",
];

/// Glyphs Claude cycles at the head of its live progress line.
///
/// It animates, so no single one of them can be required; membership in the set
/// is what identifies the line.
const PROGRESS_GLYPHS: &[char] = &['·', '*', '✢', '✳', '∗', '✻', '✽', '✶', '✴'];

/// How far up from the live tail a progress line may be found.
///
/// It is drawn immediately above the composer, so a short window is enough — and
/// necessary, because a transcript is full of retained progress lines from turns
/// that finished long ago.
const PROGRESS_TAIL_LINES: usize = 6;

/// Whether Claude's animated progress line is on the live tail of `screen`.
///
/// Current Claude Code does *not* print "esc to interrupt". It draws a spinner,
/// a gerund, and a parenthesised elapsed timer:
///
/// ```text
/// ✽ Considering… (7s · ↓ 193 tokens · thinking with medium effort)
/// ```
///
/// That mattered more than it looks: [`is_working`] is what vetoes the vague
/// cues, so a Claude whose working state we could not recognise had no veto at
/// all — and no way to say a harness was busy rather than merely alive.
///
/// Matched structurally, on the shape rather than the wording, because the
/// gerund is drawn from a long and cheerfully unstable list ("Considering",
/// "Cogitating", "Puzzling"). Three things are required together — the spinner
/// glyph, the ellipsis, and the elapsed timer — because each alone appears in
/// ordinary output and the combination does not.
fn has_live_progress_line(screen: &str) -> bool {
    screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(PROGRESS_TAIL_LINES)
        // A normal composer below a retained spinner means Claude has returned
        // control to the operator. Do not let that old line keep the session
        // marked busy forever.
        .take_while(|line| !is_composer(line))
        .any(is_progress_line)
}

/// Whether one line is a spinner-led progress line with an elapsed timer.
fn is_progress_line(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '│', '┃', '|']).trim_start();
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    PROGRESS_GLYPHS.contains(&first) && trimmed.contains('…') && has_elapsed_timer(trimmed)
}

/// Whether `line` carries a `(12s` style elapsed counter.
///
/// The digits must be followed by `s` and then the end of the group or a
/// separator, so a version string like `(2s3)` or a path fragment cannot match.
fn has_elapsed_timer(line: &str) -> bool {
    line.match_indices('(').any(|(at, _)| {
        let rest = &line[at + 1..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        let mut after = rest[digits..].chars();
        after.next() == Some('s')
            && matches!(
                after.next(),
                None | Some(' ') | Some(')') | Some('·') | Some('•')
            )
    })
}

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
        HarnessProvider::Codex,
        &["andtellcodexwhattodo"],
        AttentionKind::Approval,
        "codex is asking permission",
    ),
];

/// Screens that mean the harness stopped because something is wrong.
///
/// Read as: if the squashed screen contains the marker, the harness is blocked
/// for the stated reason. All of these are printed *instead of* a completed
/// turn — the work did not happen — and every one of them needs a person: a new
/// credential, a wait for a quota window, a retry.
///
/// Provider-agnostic on purpose. These phrases come from the model APIs rather
/// than from any one CLI's chrome, so they read the same whichever harness
/// surfaced them, and a new provider inherits the detection for free.
///
/// Deliberately narrow. "error" alone appears in every second line of ordinary
/// tool output; each marker here names a *terminal* condition and nothing that a
/// harness routinely prints while recovering on its own.
const ERRORS: &[(&str, &str)] = &[
    ("usagelimitreached", "usage limit reached"),
    ("youvehityourusagelimit", "usage limit reached"),
    ("creditbalanceistoolow", "credit balance too low"),
    ("invalidapikey", "credential rejected — needs sign-in"),
    ("oauthtokenhasexpired", "sign-in expired"),
    ("pleaserunlogin", "needs sign-in"),
    ("signinwithchatgpt", "needs sign-in"),
    ("authenticationfailed", "authentication failed"),
    ("accountdoesnothaveaccess", "account lacks access"),
];

/// Whether the harness has already recovered from an error it printed earlier.
///
/// A terminal retains everything: an expired-token message from an hour ago sits
/// in the same scrollback as the turn that succeeded after the operator signed
/// back in. Only the live tail can be a *current* error, so the search is bound
/// to it the same way the menu detectors bind theirs.
const ERROR_TAIL_LINES: usize = 12;

/// The blocking error on the live tail of `screen`, if there is one.
fn blocking_error(screen: &str) -> Option<&'static str> {
    let lines: Vec<&str> = screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail_start = lines.len().saturating_sub(ERROR_TAIL_LINES);
    let tail = &lines[tail_start..];
    let error_index = tail.iter().rposition(|line| {
        if is_composer(line) {
            return false;
        }
        let squashed = squash(line);
        ERRORS.iter().any(|(marker, _)| squashed.contains(marker))
    })?;
    // A submitted retry leaves its old composer in the transcript and restores
    // the persistent composer once the turn settles. Two composers after an
    // error therefore prove that a later turn completed; the old error no
    // longer describes the session's live state.
    if tail[error_index + 1..]
        .iter()
        .filter(|line| is_composer(line))
        .nth(1)
        .is_some()
    {
        return None;
    }
    // An error is terminal only when the harness printed it and then went
    // idle. Output below the error line that shows the harness kept going — a
    // tool that echoed "authentication failed" and continued, the tail of a
    // turn that completed anyway — proves the phrase was part of a turn's
    // output, not the reason the harness stopped. A wrapped continuation of
    // the error message itself is not such evidence: a narrow pane breaks
    // "Your limit will reset at 3pm." onto its own row, and that row must not
    // read as a recovered turn.
    if tail[error_index + 1..]
        .iter()
        .filter(|line| !is_composer(line))
        .any(|line| is_recovery_evidence(line))
    {
        return None;
    }
    // The prompt is part of the terminal viewport too, but it is operator
    // input rather than harness output. In particular, a draft such as
    // `> fix invalid API key` must not turn an otherwise idle session red.
    let tail = squash(
        &tail
            .iter()
            .copied()
            .filter(|line| !is_composer(line))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    ERRORS
        .iter()
        .find(|(marker, _)| tail.contains(marker))
        .map(|(_, what)| *what)
}

/// Whether a non-composer line below a matched error proves the harness went
/// on — a live status/activity line rather than a wrapped continuation of the
/// error text itself.
///
/// A terminal error message wraps in a narrow pane: "Claude usage limit
/// reached. Your limit will reset at 3pm." can break after "reached", leaving
/// a plain-prose continuation row that names no error marker. Such a row is
/// layout, not recovery. Status lines, by contrast, are marked: they lead with
/// an activity glyph (`✓ done`, `⏺ Working on it…`, `• Working (8s …)`) or say
/// in words that the harness is working or finished — a completed action, which
/// a wrapped error message never claims about itself.
fn is_recovery_evidence(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '│', '┃', '|']).trim_start();
    if trimmed
        .chars()
        .next()
        .is_some_and(|first| PROGRESS_GLYPHS.contains(&first) || "✓⏺•".contains(first))
    {
        return true;
    }
    let squashed = squash(trimmed);
    let lower = trimmed.to_lowercase();
    WORKING.iter().any(|marker| squashed.contains(marker))
        || [
            "working",
            "done",
            "finished",
            "completed",
            "succeeded",
            "retried",
            "retrying",
        ]
        .iter()
        .any(|word| lower.contains(word))
}

/// Whether OpenCode drew its permission action menu.
///
/// Each label alone is ordinary prose ("always allow retries", "allow once").
/// Their combination is the recognizable menu context and avoids flagging
/// retained conversation even while the working footer is visible.
fn has_opencode_permission_menu(squashed: &str) -> bool {
    squashed.contains("allowonce") && squashed.contains("alwaysallow")
}

/// Whether Claude drew the numbered menu used to leave plan mode.
///
/// "Keep planning" is ordinary conversational language, so its words alone
/// are not a safe marker. Require it to be a numbered option in the active
/// menu tail, alongside the selected-option structure that proves the harness
/// is waiting for a choice.
fn has_claude_plan_exit_menu(screen: &str) -> bool {
    let lines: Vec<&str> = screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail_start = lines.len().saturating_sub(12);
    let tail = &lines[tail_start..];
    let Some(selected) = active_selected_option_index(tail) else {
        return false;
    };
    // The selected row starts the actionable portion of the live menu. A
    // matching row above it belongs to retained output (or an older menu), so
    // it must not relabel the current choice as a plan-exit decision. Likewise,
    // a composer below the selected option means Claude accepted the menu and
    // returned to idle input; those retained options are no longer actionable.
    if tail[selected + 1..].iter().any(|line| is_composer(line)) {
        return false;
    }
    tail[selected..].iter().enumerate().any(|(offset, _)| {
        let index = selected + offset;
        numbered_option_contains(tail, index, "keepplanning")
            || numbered_option_contains(tail, index, "yesandautoacceptedits")
            || numbered_option_contains(tail, index, "yesandmanuallyapproveedits")
    })
}

/// Whether a numbered option and its wrapped continuation contain `marker`.
fn numbered_option_contains(lines: &[&str], index: usize, marker: &str) -> bool {
    let Some(label) = numbered_option_label(lines[index]) else {
        return false;
    };
    let mut joined = squash(label);
    for continuation in lines[index + 1..].iter().take(3) {
        if numbered_option_label(continuation).is_some() || is_composer(continuation) {
            break;
        }
        let continuation = continuation
            .trim_start_matches([' ', '│', '┃', '|'])
            .trim_start();
        joined.push_str(&squash(continuation));
    }
    joined.contains(marker)
}

/// Return the label portion of a numbered menu row.
fn numbered_option_label(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches([' ', '│', '┃', '|']).trim_start();
    let without_caret = trimmed
        .strip_prefix(|first| CARETS.contains(&first))
        .unwrap_or(trimmed)
        .trim_start();
    let digits: String = without_caret
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    let after = &without_caret[digits.len()..];
    matches!(after.chars().next(), Some('.') | Some(')')).then(|| after[1..].trim_start())
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
    has_live_working_marker(screen) || has_live_progress_line(screen)
}

/// Whether a known working footer remains in the live bottom-of-screen region.
///
/// A completed turn can leave its old interrupt hint in scrollback. An ordinary
/// composer below that hint establishes that control has returned to the user.
fn has_live_working_marker(screen: &str) -> bool {
    let lines: Vec<&str> = screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail_start = lines.len().saturating_sub(PROGRESS_TAIL_LINES);
    let mut composer_below = false;
    for line in lines[tail_start..].iter().rev() {
        let squashed = squash(line);
        if WORKING.iter().any(|marker| squashed.contains(marker)) {
            // A working footer above a composer is stale only when it is bare.
            // Codex keeps its composer on screen while it works, so its active
            // footer — `• Working (8s • esc to interrupt)` — carries a live
            // elapsed counter, and Claude's progress line (`✻ Reticulating
            // splines… (12s · esc to interrupt)`) is shaped the same way. A
            // retained interrupt hint in scrollback has no counter at all.
            return !composer_below || has_elapsed_timer(line);
        }
        if is_composer(line) {
            composer_below = true;
        }
    }
    false
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
    has_active_selected_option_in(&lines[tail_start..])
}

/// Whether `lines` contain a selected option with no composer below it.
fn has_active_selected_option_in(lines: &[&str]) -> bool {
    active_selected_option_index(lines).is_some()
}

/// Locate the live menu's selected row, excluding completed menus above a composer.
fn active_selected_option_index(lines: &[&str]) -> Option<usize> {
    lines.iter().enumerate().rev().find_map(|(index, line)| {
        (is_selected_option(line) && !lines[index + 1..].iter().any(|line| is_composer(line)))
            .then_some(index)
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
    let tail_start = lines.len().saturating_sub(8);
    let tail = &lines[tail_start..];
    let Some(selected) = tail.iter().position(|line| is_dialog_selected_option(line)) else {
        return false;
    };
    let footer = squash(&tail.join("\n"));
    let has_confirmation =
        footer.contains("entertoconfirm") || footer.contains("pressentertocontinue");
    has_confirmation && !tail[selected + 1..].iter().any(|line| is_composer(line))
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

/// The permission-marker cue when the menu carrying the marker is still live.
///
/// The markers are the option labels of each harness's permission menu, matched
/// on squashed text so a wrap in a narrow pane cannot hide them. But a menu the
/// operator already answered stays in scrollback above the restored composer,
/// and a whole-screen search alone would keep matching its labels — recreating
/// the approval cue after acknowledgement and leaving the row pulsing
/// indefinitely. Binding the match to the live region below the last composer
/// rejects those retained labels while keeping the wrap-tolerant whole-screen
/// match for a menu with no composer beneath it (the active case).
fn marker_cue(provider: HarnessProvider, screen: &str) -> Option<(AttentionKind, String)> {
    let lines: Vec<&str> = screen.lines().collect();
    let live = match lines.iter().rposition(|line| is_composer(line)) {
        // No composer anywhere means the harness is mid-menu: the whole screen
        // is the live region.
        None => &lines[..],
        // A composer is the current idle prompt; anything below it is its own
        // chrome, and everything above it is history that must not re-raise a
        // cue the operator already answered.
        Some(index) => &lines[index + 1..],
    };
    let squashed = squash(&live.join("\n"));
    MARKERS
        .iter()
        .filter(|(candidate, ..)| *candidate == provider)
        .find(|(_, markers, ..)| markers.iter().any(|marker| squashed.contains(marker)))
        .map(|(_, _, kind, what)| (*kind, (*what).to_string()))
}

/// What `screen` says this harness is waiting for, if anything.
///
/// Returns the cue *and* its wording; the caller stamps it with a time and
/// decides whether it displaces what the row already carries. `None` means the
/// screen shows no question we can recognise — which is the common case, and
/// deliberately not the same as "the harness is busy".
pub fn detect(provider: HarnessProvider, screen: &str) -> Option<(AttentionKind, String)> {
    // A harness that is plainly mid-turn wants nothing: retained prompts and
    // error text must not outlive the active footer as a stale attention cue.
    if is_working(screen) {
        return None;
    }
    // A recognised startup dialog outranks everything: it is the one case where
    // the harness will not take work at all, and it already has wording written
    // for an operator.
    let dialog = has_active_dialog_context(screen)
        .then(|| blocking_dialog_for(provider, screen))
        .flatten();
    if let Some(dialog) = dialog {
        return Some((AttentionKind::Dialog, dialog.what.to_string()));
    }

    let squashed = squash(screen);
    if let Some(cue) = marker_cue(provider, screen) {
        return Some(cue);
    }
    if provider == HarnessProvider::Claude && has_claude_plan_exit_menu(screen) {
        return Some((
            AttentionKind::Approval,
            "claude finished planning and wants a decision".to_string(),
        ));
    }
    if provider == HarnessProvider::Opencode && has_opencode_permission_menu(&squashed) {
        return Some((
            AttentionKind::Approval,
            "opencode is asking permission".to_string(),
        ));
    }

    // A blocking error outranks the structural fallbacks below it and is checked
    // after the prompts above: a harness can print "usage limit reached" and then
    // ask what to do about it, and the question is the more useful thing to say.
    //
    if let Some(what) = blocking_error(screen) {
        return Some((
            AttentionKind::Error,
            format!("{} stopped: {what}", provider.as_str()),
        ));
    }

    // Unrecognised wording, recognisable shape.
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
