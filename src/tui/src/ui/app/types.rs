//! The data model for the interactive TUI screen: the tab list and Settings
//! subpage constants, the [`Cmd`] the event loop runs on the app's behalf, the
//! small overlay/state types ([`ResumePicker`], [`Prompt`], [`PromptKind`],
//! [`MemoryEntry`]), and the central [`App`] struct itself.
//!
//! Behaviour lives in the sibling modules ([`super::state`], [`super::input`],
//! [`super::keys`], [`super::commands`], and [`super::render`]), each of which
//! adds its own `impl App` block. Because those blocks share `App`'s private
//! fields, the fields (and the private helper types/consts here) are
//! `pub(super)` so every sibling submodule can reach them.

use std::sync::Arc;

use ratatui::layout::Rect;

use crate::ui::composer::Draft;
use crate::ui::theme::Theme;
use medulla::client::{FeedbackComment, FeedbackItem, FeedbackQuery, FeedbackType};
use medulla::config::LoadedConfig;
use medulla::memory::{MemoryHit, MemoryStatus};
use medulla::runtime::{ContextItem, Runtime, RuntimeSnapshot, WorkerOp};

/// The ordered top-level tab names. The tab index selects into this array.
///
/// Trace, Context, and Feedback used to live here. They are secondary surfaces —
/// two of them diagnostic — so they now sit under Settings, keeping the tab bar
/// to the views a session is actually driven from.
pub const TABS: [&str; 7] = [
    "Overview", "Chat", "Agents", "Repo", "Workers", "Memory", "Settings",
];

/// The Settings tab's left-nav subpages, in order (number keys 1-8 jump to them).
///
/// This is the flat, selectable list [`App::settings_index`] indexes into.
/// [`SETTINGS_GROUPS`] overlays the display-only headings.
pub const SETTINGS_SUBPAGES: [&str; 8] = [
    "Usage",
    "Appearance",
    "Config",
    "Feedback",
    "Trace",
    "Context",
    "Account",
    "Help",
];

/// The left-nav group headings, as `(heading, first subpage index)`.
///
/// Headings are rendered dim and are not selectable — they exist to separate the
/// everyday settings from the diagnostic ones. Each group runs until the next
/// group's start index.
pub const SETTINGS_GROUPS: [(&str, usize); 3] = [
    ("GENERAL", SP_USAGE),
    ("DEBUG", SP_TRACE),
    ("ABOUT", SP_ACCOUNT),
];

// Settings subpage indices.
pub(super) const SP_USAGE: usize = 0;
pub(super) const SP_APPEARANCE: usize = 1;
pub(super) const SP_CONFIG: usize = 2;
pub(super) const SP_FEEDBACK: usize = 3;
pub(super) const SP_TRACE: usize = 4;
pub(super) const SP_CONTEXT: usize = 5;
pub(super) const SP_ACCOUNT: usize = 6;
pub(super) const SP_HELP: usize = 7;

/// The index of a tab by name, or 0 if unknown. Keeps tab jumps robust as the tab
/// list grows.
pub(super) fn tab_pos(name: &str) -> usize {
    TABS.iter().position(|t| *t == name).unwrap_or(0)
}

/// An async action the event loop must run on the app's behalf.
#[derive(Debug)]
pub enum Cmd {
    /// Exit the application.
    Quit,
    /// Submit a composer line as a new conversational turn.
    Submit(String),
    /// Resume a previously saved chat by session id.
    Resume(String),
    /// Fetch the list of resumable chats for the resume picker.
    ListChats,
    /// Re-inspect the runtime's context chunks for the Context tab.
    InspectContext,
    /// Refresh all configured local Git workspace summaries.
    LoadWorkspaces(Vec<std::path::PathBuf>),
    /// Load the patch for one selected repository-relative path.
    LoadWorkspaceDiff {
        /// Local repository root.
        workspace: std::path::PathBuf,
        /// Repository-relative changed path.
        path: std::path::PathBuf,
    },
    /// Collect the lane-scoped Git diff and submit an independent review task.
    PrepareReview {
        task_id: String,
        implementer_id: String,
        reviewer_id: String,
        workspace: std::path::PathBuf,
        contract: medulla::autoreview::ReviewContract,
    },
    /// Apply a worker fleet mutation.
    WorkerOp(WorkerOp),
    /// Load the persona-memory status + directives for the Memory tab.
    LoadMemory,
    /// Fetch account-level usage from the backend for the Usage tab.
    LoadUsage,
    /// Run a persona-memory search and land on the Memory tab.
    SearchMemory(String),
    /// Run a persona-memory ingest, then reload the Memory tab. `backfill` walks
    /// everything oldest-first; otherwise only changed files/repos are visited.
    IngestMemory {
        /// Whether to walk everything rather than resuming from the cursor.
        backfill: bool,
    },
    /// Load a page of the feedback board for the Feedback tab.
    LoadFeedback(FeedbackQuery),
    /// Load one board item's comments for the detail pane.
    LoadFeedbackDetail(String),
    /// Cast, change, or retract a vote on a board item.
    VoteFeedback {
        /// The item being voted on.
        id: String,
        /// `1` upvote, `-1` downvote, `0` retract.
        value: i8,
    },
    /// Post a comment on a board item.
    CommentFeedback {
        /// The item being commented on.
        id: String,
        /// The comment text.
        body: String,
    },
    /// Submit new feedback to the board.
    SubmitFeedback {
        /// Feature request or bug report.
        kind: FeedbackType,
        /// The submission's title.
        title: String,
        /// The submission's body.
        body: String,
    },
}

/// The modal state for the "resume a chat" picker overlay.
pub(super) struct ResumePicker {
    /// The resumable chats to choose from.
    pub(super) chats: Vec<crate::ui::chat_store::MainChatSummary>,
    /// The highlighted row.
    pub(super) index: usize,
}

/// One selectable row in the Memory tab's left pane: either the directive/facet
/// overview (no active search) or a ranked search hit.
pub(super) enum MemoryEntry {
    /// A persona directive line.
    Directive(String),
    /// A facet name with its observation count.
    Facet {
        /// The facet name.
        name: String,
        /// The number of observations in the facet.
        count: usize,
    },
    /// A ranked search hit.
    Hit(MemoryHit),
}

/// The action a small inline prompt (Workers add/edit, Agents answer) submits.
pub(super) enum PromptKind {
    /// Add a worker from an address/@handle line.
    WorkerAdd,
    /// Edit the label of the worker with the given id.
    WorkerEditLabel(String),
    /// Answer a pending sub-agent question.
    AnswerQuestion {
        /// The cycle the question belongs to.
        cycle_id: String,
        /// The pending question's id.
        question_id: String,
    },
    /// Comment on the given feedback board item.
    FeedbackComment {
        /// The item being commented on.
        id: String,
    },
    /// Step one of submitting feedback: the title. Submitting advances to
    /// [`PromptKind::FeedbackBody`] rather than sending anything.
    FeedbackTitle {
        /// Feature request or bug report, chosen by which key opened the prompt.
        kind: FeedbackType,
    },
    /// Step two of submitting feedback: the body. Submitting sends it.
    FeedbackBody {
        /// Feature request or bug report.
        kind: FeedbackType,
        /// The title captured in step one.
        title: String,
    },
}

/// The Feedback tab's state: the loaded page, the selected row, that row's
/// comments, and the active query.
pub(super) struct FeedbackState {
    /// The current page of board items.
    pub(super) items: Vec<FeedbackItem>,
    /// Total items matching the query across all pages.
    pub(super) total: i64,
    /// The highlighted row.
    pub(super) index: usize,
    /// Comments for [`FeedbackState::detail_id`], loaded lazily on selection.
    pub(super) comments: Vec<FeedbackComment>,
    /// Which item [`FeedbackState::comments`] belongs to.
    pub(super) detail_id: Option<String>,
    /// Scroll offset within the detail pane.
    pub(super) detail_scroll: usize,
    /// The active filter/sort/pagination.
    pub(super) query: FeedbackQuery,
    /// Whether the runtime serves a board at all. `false` renders a sign-in
    /// hint instead of an empty list.
    pub(super) supported: bool,
    /// Whether a board load is in flight (drives the header's "loading…").
    pub(super) loading: bool,
}

impl Default for FeedbackState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            total: 0,
            index: 0,
            comments: Vec::new(),
            detail_id: None,
            detail_scroll: 0,
            query: FeedbackQuery::default(),
            supported: true,
            loading: false,
        }
    }
}

/// Repo-tab summaries, selection, and the lazily loaded selected patch.
#[derive(Default)]
pub(super) struct RepoState {
    /// Best-effort report for every configured workspace.
    pub(super) reports: Vec<medulla::workspace::WorkspaceReport>,
    /// Selection in the flattened dirty-file list.
    pub(super) file_index: usize,
    /// Workspace/path the current patch belongs to.
    pub(super) diff_key: Option<(std::path::PathBuf, std::path::PathBuf)>,
    /// Current selected-file patch.
    pub(super) diff: String,
    /// Diff-pane error, kept separate so repository summaries stay visible.
    pub(super) diff_error: Option<String>,
    /// Vertical line offset in the patch pane.
    pub(super) diff_scroll: usize,
    /// Whether a workspace refresh is in flight.
    pub(super) loading: bool,
}

/// A single-line inline input overlay, composer-styled, reused for the fleet and
/// steering prompts.
pub(super) struct Prompt {
    /// What the prompt submits.
    pub(super) kind: PromptKind,
    /// The overlay title.
    pub(super) title: String,
    /// The editable draft buffer.
    pub(super) draft: Draft,
}

/// The interactive TUI screen: all tab state, input focus, and render geometry.
pub struct App {
    /// The runtime this screen drives.
    pub runtime: Arc<dyn Runtime>,
    /// The loaded configuration (for the Config/Overview surfaces).
    pub loaded: LoadedConfig,
    /// The most recent runtime snapshot, refreshed each loop tick.
    pub snapshot: RuntimeSnapshot,
    /// The active top-level tab index (into [`TABS`]).
    pub tab_index: usize,
    pub(super) draft: Draft,
    pub(super) history: Vec<String>,
    pub(super) history_index: i64,
    pub(super) selected: usize,
    pub(super) status: String,
    /// A persistent "update vX.Y.Z available" banner, set by the background
    /// update checker; shown in the header until the app exits.
    pub(super) update_notice: Option<String>,
    pub(super) contexts: Vec<ContextItem>,
    pub(super) context_index: usize,
    pub(super) agent_index: usize,
    pub(super) agent_scroll: usize,
    pub(super) chat_scroll: usize,
    pub(super) worker_index: usize,
    // Persona-memory tab state (lazily loaded on tab entry / search).
    pub(super) memory_status: Option<MemoryStatus>,
    pub(super) memory_hits: Vec<MemoryHit>,
    pub(super) memory_directives: Vec<String>,
    pub(super) memory_index: usize,
    pub(super) memory_query: Option<String>,
    /// The persona-memory service, attached directly rather than through the
    /// runtime seam. Memory is a local, on-disk surface that has nothing to do
    /// with which runtime drives chat, so attaching it here keeps the Memory tab
    /// working on the backend and mock paths — not just on core, which is the
    /// only runtime that also *serves* memory as a toolset. `None` falls back to
    /// the runtime seam (how the mock scripts memory in tests).
    pub(super) memory_service: Option<Arc<medulla::memory::MemoryService>>,
    /// Whether a memory ingest (backfill or incremental) is currently running.
    /// Ingest calls a paid provider, so a second run must not be startable while
    /// one is in flight.
    pub(super) memory_ingesting: bool,
    /// Feedback-board tab state (lazily loaded on tab entry / refresh).
    pub(super) feedback: FeedbackState,
    /// Local Git ledger state for the Repo tab.
    pub(super) repo: RepoState,
    pub(super) prompt: Option<Prompt>,
    /// The animation frame counter (drives the spinner).
    pub frame: usize,
    /// Whether the app currently captures the mouse.
    pub mouse_capture: bool,
    /// Account-level usage payload (`/teams/me/usage` data), when fetched.
    pub account_usage: Option<serde_json::Value>,
    /// The active Settings subpage (index into [`SETTINGS_SUBPAGES`]).
    pub(super) settings_index: usize,
    /// Whether keyboard focus is inside the Settings content pane rather than on
    /// the left-hand subpage nav.
    ///
    /// Subpages whose content is a list of *actions* (Feedback especially) bind
    /// enough single letters that they swallow the keys you would otherwise use
    /// to get around, and `↑↓` moving the nav meant arrow keys jumped you off
    /// the page entirely. Entering the pane hands `↑↓` to the content and makes
    /// the letter bindings deliberate rather than ambient.
    pub(super) settings_focused: bool,
    /// The selected theme role on the Appearance subpage.
    pub(super) appearance_index: usize,
    /// The selected editable row on the Config subpage.
    pub(super) config_index: usize,
    /// Whether the Account subpage's logout is armed. Logging out clears stored
    /// credentials, so the first Enter arms and the second confirms; any other
    /// navigation disarms it.
    pub(super) logout_armed: bool,
    /// Whether the app is quitting in order to re-authenticate rather than to
    /// exit. Set by a successful logout so the caller tears the session down and
    /// returns to the login screen instead of returning to the shell.
    pub(super) relogin_requested: bool,
    /// The Medulla home directory, used to locate the credential store the
    /// Account subpage clears. Injectable so feature tests never touch the real
    /// home; `None` disables logout.
    pub(super) medulla_home: Option<std::path::PathBuf>,
    /// The resolved color theme; selection highlighting + chrome draw from it.
    pub(super) theme: Theme,
    /// Where appearance changes are persisted (the user-global `config.toml`).
    /// Injectable so feature tests never touch the real home. `None` disables
    /// persistence (changes still apply live).
    pub(super) config_path: Option<std::path::PathBuf>,
    pub(super) resume_picker: Option<ResumePicker>,
    /// Whether the event loop should exit after this tick.
    pub should_quit: bool,

    // Render geometry, recorded each draw for click hit-testing.
    pub(super) area: Rect,
    pub(super) hit_tabs: Vec<(u16, u16)>,
    pub(super) hit_tabs_row: u16,
    pub(super) hit_agents: Option<(Rect, usize)>,
    pub(super) hit_context: Option<Rect>,
    pub(super) hit_threads: Option<(Rect, usize)>,
    pub(super) last_events_len: usize,

    // Test-only clipboard capture: when set, `copy_chat` records the copied text
    // here and skips the platform writers (no `pbcopy`/OSC subprocess in tests).
    pub(super) copy_capture: Option<Arc<std::sync::Mutex<Vec<String>>>>,

    // Optional observational overlay from the background tinyplace service:
    // this TUI's own identity, its peer roster, and peer presence. Merged into
    // the snapshot on every refresh so the Overview panel and Agents lanes light
    // up without the runtime having to know about tiny.place.
    pub(super) tinyplace_obs:
        Option<Arc<std::sync::Mutex<medulla::tinyplace::service::TinyplaceObservation>>>,
}
