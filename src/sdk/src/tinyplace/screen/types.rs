//! Data model for the `medulla.screen.v1` wire protocol: the synchronised
//! terminal state, the frames that carry it, and their serde shapes.
//!
//! These types are deliberately independent of `vt100`. The SDK stays free of
//! the terminal-emulator and pty crates (that wiring lives in the app crate), so
//! a screen crossing the wire is described here in its own vocabulary and the
//! app crate converts at the boundary.

use serde::{Deserialize, Serialize};

/// Wire version tag stamped on every screen message body.
pub const SCREEN_PROTO: &str = "medulla.screen.v1";

/// Bold attribute bit for [`RunStyle::attrs`].
pub const ATTR_BOLD: u8 = 1 << 0;
/// Italic attribute bit for [`RunStyle::attrs`].
pub const ATTR_ITALIC: u8 = 1 << 1;
/// Underline attribute bit for [`RunStyle::attrs`].
pub const ATTR_UNDERLINE: u8 = 1 << 2;
/// Inverse-video attribute bit for [`RunStyle::attrs`].
pub const ATTR_INVERSE: u8 = 1 << 3;

/// A cell colour.
///
/// Mirrors the three forms a terminal emulator reports — inherit, palette index,
/// or direct RGB — without depending on the emulator's own type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    /// Inherit the viewer's own palette. Never forced to a colour we picked.
    #[default]
    Default,
    /// An index into the 256-colour palette.
    Idx(u8),
    /// A direct 24-bit colour.
    Rgb(u8, u8, u8),
}

impl Color {
    /// Whether this is the inherit-from-viewer default.
    ///
    /// Used to keep default colours out of the serialized frame, which is most
    /// of a typical screen.
    pub fn is_default(&self) -> bool {
        matches!(self, Color::Default)
    }
}

/// The visual style shared by a run of adjacent cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RunStyle {
    /// Foreground colour.
    #[serde(default, skip_serializing_if = "Color::is_default")]
    pub fg: Color,
    /// Background colour.
    #[serde(default, skip_serializing_if = "Color::is_default")]
    pub bg: Color,
    /// Bitset of `ATTR_*` flags.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attrs: u8,
}

/// Whether an attribute bitset is empty, so it can be omitted from the wire.
fn is_zero(value: &u8) -> bool {
    *value == 0
}

impl RunStyle {
    /// Whether `flag` (an `ATTR_*` constant) is set.
    pub fn has(&self, flag: u8) -> bool {
        self.attrs & flag != 0
    }
}

/// A run of adjacent cells sharing one style.
///
/// Rows are carried as runs rather than cells because a terminal row is mostly
/// long stretches of one style: a 120-column row is typically two or three runs,
/// not 120 cells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRun {
    /// The run's text.
    #[serde(rename = "t")]
    pub text: String,
    /// The style every cell in the run shares.
    #[serde(flatten)]
    pub style: RunStyle,
}

impl ScreenRun {
    /// Build a run from its text and style.
    pub fn new(text: impl Into<String>, style: RunStyle) -> Self {
        ScreenRun {
            text: text.into(),
            style,
        }
    }

    /// Build an unstyled run.
    pub fn plain(text: impl Into<String>) -> Self {
        ScreenRun::new(text, RunStyle::default())
    }
}

/// One row replaced wholesale.
///
/// Rows are the diff unit: a changed row is resent entire rather than as a
/// within-row patch. A terminal repaints by line, so sub-row deltas would cost
/// more to describe than the row costs to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowUpdate {
    /// Zero-based row index, from the top of the screen.
    pub y: u16,
    /// The row's full new contents.
    pub runs: Vec<ScreenRun>,
}

/// A complete terminal screen: the state this protocol synchronises.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScreenGrid {
    /// Width in cells.
    pub cols: u16,
    /// Height in cells.
    pub rows: u16,
    /// One entry per row, top to bottom. Always `rows` long for a well-formed
    /// grid.
    pub lines: Vec<Vec<ScreenRun>>,
    /// Cursor position as `(row, col)`.
    pub cursor: (u16, u16),
    /// Whether the harness has hidden its cursor.
    pub hide_cursor: bool,
}

impl ScreenGrid {
    /// Whether `other` has the same dimensions.
    ///
    /// Row indices are only meaningful between grids of one size, so this gates
    /// whether a diff may be taken at all.
    pub fn same_size(&self, other: &ScreenGrid) -> bool {
        self.cols == other.cols && self.rows == other.rows
    }
}

/// A screen frame: either a whole grid or the rows that changed since the state
/// the sender believes the receiver holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenFrame {
    /// The task whose session this frame shows.
    ///
    /// Deliberately the *task* id and not the worker's session id. A session
    /// serves one task at a time — the worker's `claim_idle` refuses a busy one
    /// under a single lock — so the two identify the same thing while a task
    /// runs, and the task id is the one both ends already hold. It also makes
    /// authorization structural: a worker resolves a task by
    /// `(authenticated sender, task id)`, so a peer can only ever name its own.
    pub task_id: String,
    /// Monotonic sequence number for the state this frame produces.
    pub seq: i64,
    /// The sequence number this frame is a diff *from*.
    ///
    /// Mosh's `ack_num`. Ignored when `full` is set; otherwise the receiver must
    /// hold exactly this sequence or ask to resynchronise.
    pub base_seq: i64,
    /// Whether `rows_changed` is the entire screen rather than a delta.
    pub full: bool,
    /// The sender's width in cells. Authoritative: the viewer adapts.
    pub cols: u16,
    /// The sender's height in cells.
    pub rows: u16,
    /// Cursor position as `(row, col)`.
    pub cursor: (u16, u16),
    /// Whether the harness has hidden its cursor.
    pub hide_cursor: bool,
    /// The rows this frame replaces — every row when `full`, otherwise only
    /// those that differ.
    pub rows_changed: Vec<RowUpdate>,
}

/// Everything that can cross this protocol, in both directions.
///
/// There is no `input` and no `resize`: the viewer cannot steer the session,
/// and the sender's geometry is authoritative. The one control operation is an
/// explicit, task-scoped kill used by an operator to recover a hung harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScreenMessage {
    /// Viewer → sender: start (or restart) a stream.
    Subscribe {
        /// The task whose session to watch.
        task_id: String,
        /// The most frames per second the viewer wants.
        max_fps: u8,
        /// Ask for a full frame rather than a delta — sent on first subscribe
        /// and whenever the viewer's state has diverged.
        resync: bool,
    },
    /// Viewer → sender: stop streaming this session.
    Unsubscribe {
        /// The task to stop watching.
        task_id: String,
    },
    /// Viewer → sender: kill the harness serving an owned running task.
    Kill {
        /// The task whose harness should be killed.
        task_id: String,
        /// The unique dispatch receipt, preventing a delayed kill from matching
        /// a later dispatch that reused the task id.
        correlation_id: String,
    },
    /// Viewer → sender: the highest sequence the viewer holds, which the next
    /// diff may be taken from.
    Ack {
        /// The task being acknowledged.
        task_id: String,
        /// The sequence the viewer now holds.
        seq: i64,
    },
    /// Sender → viewer: new screen state.
    Frame(ScreenFrame),
}

/// A screen message with its version tag, as it appears on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenEnvelope {
    /// Wire version tag ([`SCREEN_PROTO`]).
    pub screen_version: String,
    /// The message itself.
    #[serde(flatten)]
    pub message: ScreenMessage,
}

/// The viewer's copy of a session's screen, and how current it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenView {
    /// The synchronised screen.
    pub grid: ScreenGrid,
    /// The sequence number `grid` reflects.
    pub seq: i64,
}

/// What the sampler decided to put on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameDecision {
    /// The screen is byte-identical to the last frame sent. Send nothing — this
    /// is what decouples wire cost from how much the harness is repainting.
    Unchanged,
    /// Send this frame.
    Send(ScreenFrame),
}

/// The result of folding a frame into a viewer's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The frame was applied; the view is current.
    Applied,
    /// The frame could not be applied against the state held. The viewer must
    /// send `subscribe { resync: true }` and wait for a full frame.
    NeedsResync,
}
