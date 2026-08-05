//! Laying out one operator-started harness row from the operator's own
//! [`StatusLineConfig`].
//!
//! The row used to be a fixed sentence — glyph, provider, control, branch,
//! path — and this module is what turned it into a layout. Every field declares
//! where it sits and how it is spelled; this decides what actually fits.
//!
//! Two rules survive from the fixed version and are the reason the arithmetic
//! here is not just "join with a separator":
//!
//! - **The row never exceeds its width.** The rail is capped at
//!   [`RAIL_MAX_CONTENT`](super::super::RAIL_MAX_CONTENT) columns precisely so one long
//!   working directory cannot push the transcript into a gutter, and a row that
//!   overran that cap would put the cost back.
//! - **The path spends what is left, and loses its head rather than its tail.**
//!   The tail names the checkout; the head is what every sibling harness on the
//!   machine already shares.
//!
//! Kept apart from [`super`] so that file stays about *which* rows the rail
//! draws rather than how one of them is measured.

use medulla::config::{
    ControlStyle, FieldPlacement, FieldVisibility, HarnessNameStyle, PathStyle, StatusLineConfig,
};
use ratatui::style::Style;
use ratatui::text::{Line as TLine, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ui::util::slug;
use crate::worker::pty::{HarnessControl, PtyState, SessionRow};

use super::super::wrap::{short_home, wrap_path};

use super::types::{Field, HarnessLineStyle};

/// The most lines one harness row may occupy.
///
/// Three, and not configurable: the rail is a list beside the surface the
/// operator is actually reading, and a row that can grow without bound turns
/// that list into a second transcript.
pub(super) const MAX_LINES: usize = 3;

/// How far a continuation line is indented, so a multi-line row still reads as
/// one entry. Two columns puts it under the provider name rather than under the
/// state glyph.
const CONT_INDENT: &str = "  ";

/// The separator between two fields on the same line.
const SEPARATOR: &str = " · ";

/// The widest a branch name is ever allowed to be.
///
/// A branch is one identifier among four fields; a long ticket-numbered branch
/// left alone would eat the working directory whole.
const BRANCH_MAX: usize = 16;

/// Columns held back from the branch for a working directory sharing its line.
///
/// Enough for `…/name`, which is the least a path can say and still identify a
/// checkout. Without it a long branch renders first and leaves the path nothing.
const PATH_RESERVE: usize = 7;

/// The least room a working directory is drawn in at all.
const PATH_MIN: usize = 4;

/// Every field, in draw order.
const ORDER: [Field; 6] = [
    Field::State,
    Field::Harness,
    Field::Control,
    Field::Thread,
    Field::Branch,
    Field::Path,
];

impl Field {
    /// Where this field sits, per the operator's config.
    fn placement(self, cfg: &StatusLineConfig) -> FieldPlacement {
        match self {
            Field::State => cfg.state,
            Field::Harness => cfg.harness,
            Field::Control => cfg.control,
            Field::Thread => cfg.thread,
            Field::Branch => cfg.branch,
            Field::Path => cfg.path,
        }
    }

    /// When this field is drawn, per the operator's config.
    fn visibility(self, cfg: &StatusLineConfig) -> FieldVisibility {
        match self {
            Field::State => cfg.state_when,
            Field::Harness => cfg.harness_when,
            Field::Control => cfg.control_when,
            Field::Thread => cfg.thread_when,
            Field::Branch => cfg.branch_when,
            Field::Path => cfg.path_when,
        }
    }

    /// Whether this field is drawn dim, as secondary detail.
    ///
    /// The glyph, provider, and control state say what the session *is*; the
    /// branch and path say where it is pointed. Dimming the latter is what lets
    /// a column of harness rows be scanned for the former.
    fn is_detail(self) -> bool {
        matches!(self, Field::Thread | Field::Branch | Field::Path)
    }
}

/// Lay out one harness row as the lines it occupies, within `width` columns.
///
/// Returns at least one line — possibly empty, if the operator has hidden every
/// field — because the rail maps drawn lines back to the row they came from and
/// a row contributing no lines could not be clicked. Never returns more than
/// [`MAX_LINES`].
///
/// `home` is passed rather than read from the environment so the shortening rule
/// stays a pure function of its inputs: this is presentation logic, and a
/// renderer that consults the process environment cannot be tested on a machine
/// that disagrees with the fixture.
pub(in crate::ui::app::render::agents::rail) fn harness_lines(
    cfg: &StatusLineConfig,
    row: &SessionRow,
    home: Option<&str>,
    render: HarnessLineStyle,
) -> Vec<TLine<'static>> {
    let shown = |field: Field| {
        field
            .visibility(cfg)
            .shows(render.active, render.alerting || needs_attention(row))
    };
    let mut lines: Vec<TLine<'static>> = Vec::new();
    for index in 0..MAX_LINES {
        let indent = if index == 0 { "" } else { CONT_INDENT };
        let spans = layout_line(
            cfg,
            row,
            home,
            &shown,
            index,
            render.width,
            indent,
            render.state_glyph,
            render.primary,
            render.detail,
        );
        // A line nothing was assigned to is closed up rather than drawn blank:
        // putting one field on line 3 and none on line 2 should cost the rail
        // two rows, not three.
        if !spans.is_empty() {
            lines.push(TLine::from(spans));
        }
    }
    // Every field hidden. The rail maps drawn lines back to the row they came
    // from, so a row contributing none could not be selected or clicked.
    if lines.is_empty() {
        lines.push(TLine::default());
    }
    lines
}

/// Build the spans for one line of a harness row.
///
/// Fields are laid out left to right and each is given only the columns still
/// free, so a field that does not fit is dropped rather than overrunning. The
/// working directory is deliberately last: it is the one field with something
/// useful to do with a variable amount of room.
#[allow(clippy::too_many_arguments)]
fn layout_line(
    cfg: &StatusLineConfig,
    row: &SessionRow,
    home: Option<&str>,
    shown: &dyn Fn(Field) -> bool,
    line: usize,
    width: usize,
    indent: &str,
    state_glyph: char,
    style: Style,
    detail_style: Style,
) -> Vec<Span<'static>> {
    let on_this_line = |field: Field| field.placement(cfg).line() == Some(line) && shown(field);
    let path_shares_line = on_this_line(Field::Path);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    // Set once the state glyph has been drawn, so the field after it is joined
    // with a plain space — `● codex`, not `● · codex`.
    let mut after_state = false;

    for field in ORDER {
        if !on_this_line(field) {
            continue;
        }
        let separator = if spans.is_empty() {
            indent
        } else if after_state {
            " "
        } else {
            SEPARATOR
        };
        after_state = field == Field::State;
        let room = width.saturating_sub(used + separator.width());

        let text = match field {
            Field::State => fit(&state_glyph.to_string(), room),
            Field::Harness => fit(&harness_text(row, cfg.harness_style), room),
            Field::Control => fit(&control_text(row.control, cfg.control_style), room),
            // The harness advertises a sentence; the status line, like the
            // lane rows, shows its slug so one row cannot eat the rail.
            Field::Thread => row
                .thread_name
                .as_deref()
                .map(slug)
                .filter(|name| !name.is_empty())
                .and_then(|name| fit(&name, room)),
            Field::Branch => branch_text(row, room, path_shares_line),
            Field::Path => path_text(row, home, cfg.path_style, room),
        };
        let Some(text) = text else { continue };

        used += separator.width() + text.width();
        spans.push(Span::styled(
            format!("{separator}{text}"),
            if field.is_detail() {
                detail_style
            } else {
                style
            },
        ));
    }
    spans
}

/// A fixed-width field clipped to the room it has, or `None` when it has none.
fn fit(text: &str, room: usize) -> Option<String> {
    (room > 0).then(|| clip_cells(text, room))
}

/// The provider name in the operator's chosen spelling.
fn harness_text(row: &SessionRow, style: HarnessNameStyle) -> String {
    match style {
        HarnessNameStyle::Long => row.provider.display_name().to_string(),
        HarnessNameStyle::Short => row.provider.as_str().to_string(),
        HarnessNameStyle::Icon => row.provider.icon().to_string(),
    }
}

/// Who holds the session, in the operator's chosen spelling.
///
/// "unmanaged" rather than [`HarnessControl::as_str`]'s "you": this is the whole
/// reason an operator-started row exists, and someone who hands one to the
/// orchestrator needs to see that it took effect.
fn control_text(control: HarnessControl, style: ControlStyle) -> String {
    match (style, control) {
        (ControlStyle::Text, HarnessControl::User) => "unmanaged",
        (ControlStyle::Text, HarnessControl::Orchestrator) => "orchestrator",
        // `⊘` reads as "dispatch does not enter here", which is exactly what an
        // operator-held session means; `⊙` is the orchestrator holding it.
        (ControlStyle::Icon, HarnessControl::User) => "⊘",
        (ControlStyle::Icon, HarnessControl::Orchestrator) => "⊙",
    }
    .to_string()
}

/// The branch name, capped and clipped, or `None` when there is no branch or no
/// usable room for one.
fn branch_text(row: &SessionRow, room: usize, path_shares_line: bool) -> Option<String> {
    let branch = row.branch.as_deref()?;
    let reserve = if path_shares_line { PATH_RESERVE } else { 0 };
    let budget = room.saturating_sub(reserve).min(BRANCH_MAX);
    // Below two columns all that fits is the ellipsis, which says nothing.
    (budget >= 2).then(|| clip_cells(branch, budget))
}

/// The working directory in the operator's chosen spelling, or `None` when it
/// has too little room to say anything.
fn path_text(
    row: &SessionRow,
    home: Option<&str>,
    style: PathStyle,
    room: usize,
) -> Option<String> {
    if room < PATH_MIN {
        return None;
    }
    Some(match style {
        // Clipped from the front: the tail names the checkout, and several rows
        // all reading `/Users/someone/work/…` would distinguish nothing.
        PathStyle::Full => clip_left_cells(&row.cwd, room),
        PathStyle::Shortened => {
            let path = short_home(&row.cwd, home);
            wrap_path(&path, room, 1).concat()
        }
        PathStyle::Last => clip_left_cells(last_segment(&row.cwd), room),
    })
}

/// Collapses whitespace and clips the right edge to `width` terminal cells.
///
/// Status-line values come from provider names, branches, and paths, all of
/// which may contain wide glyphs. Counting Unicode scalars would allow a CJK
/// value to consume twice its assigned rail budget.
fn clip_cells(value: &str, width: usize) -> String {
    let single = single_line(value);
    if single.width() <= width {
        return single;
    }
    let budget = width.saturating_sub(1);
    let mut used = 0;
    let mut out = String::new();
    for character in single.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > budget {
            break;
        }
        used += character_width;
        out.push(character);
    }
    out.push('…');
    out
}

/// Collapses whitespace and clips the left edge to `width` terminal cells.
fn clip_left_cells(value: &str, width: usize) -> String {
    let single = single_line(value);
    if single.width() <= width {
        return single;
    }
    let budget = width.saturating_sub(1);
    let mut used = 0;
    let mut start = single.len();
    for (index, character) in single.char_indices().rev() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > budget {
            break;
        }
        used += character_width;
        start = index;
    }
    format!("…{}", &single[start..])
}

/// Normalizes a user-controlled value into the one-line form the rail draws.
fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether this session is in a state an operator would want flagged.
///
/// What [`FieldVisibility::Alert`] keys off: the harness died, its child exited
/// non-zero, or it recorded an error. Deliberately not "busy" — busy is the
/// normal case, and a field that shows on every working harness is a field that
/// shows always.
pub(super) fn needs_attention(row: &SessionRow) -> bool {
    row.last_error.is_some()
        || matches!(row.state, PtyState::Failed)
        || matches!(row.state, PtyState::Exited { code: Some(code) } if code != 0)
}

/// The final component of a path — usually the directory the harness runs in.
///
/// Falls back to the whole value when there is no separator to split on, and to
/// the separator itself for a filesystem root, so this never returns an empty
/// string for a non-empty path.
fn last_segment(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return if path.is_empty() { path } else { "/" };
    }
    match trimmed.rsplit_once(['/', '\\']) {
        Some((_, tail)) if !tail.is_empty() => tail,
        _ => trimmed,
    }
}
