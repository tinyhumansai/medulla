//! Structural recognition for Claude Code prompts whose wording changes often.
//!
//! Claude 2.1 shortened its permission choices to plain `Yes` and `No` while
//! leaving the active tool line labelled `Waiting…`. That combination defeats
//! both the older phrase marker and the generic working veto, so the numbered
//! confirmation is recognised as one atomic screen shape here.

use super::detect::{is_composer, is_selected_option};

/// Whether Claude's live tail is a numbered tool-permission confirmation.
pub(super) fn has_permission_menu(screen: &str) -> bool {
    let lines: Vec<&str> = screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail_start = lines.len().saturating_sub(18);
    let tail = &lines[tail_start..];
    let Some(selected) = tail.iter().rposition(|line| is_selected_option(line)) else {
        return false;
    };
    if tail[selected + 1..].iter().any(|line| is_composer(line)) {
        return false;
    }

    let squashed: String = tail
        .iter()
        .flat_map(|line| line.chars())
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let confirmation = squashed.contains("doyouwanttoproceed")
        && squashed.contains("1yes")
        && squashed.contains("3no");
    let plan_menu = squashed.contains("keepplanning")
        || squashed.contains("autoacceptedits")
        || squashed.contains("revieweachedit");
    confirmation && !plan_menu
}
