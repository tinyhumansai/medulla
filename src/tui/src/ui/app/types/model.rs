//! The data model for the interactive TUI screen: the tab list, multi-pane
//! navigation constants, the [`Cmd`] the event loop runs on the app's behalf, the
//! small overlay/state types ([`ResumePicker`], [`Prompt`], [`PromptKind`],
//! and the central [`App`] struct itself.
//!
//! Behaviour lives in the sibling modules ([`super::super::state`], [`super::super::input`],
//! [`super::super::keys`], [`super::super::commands`], and [`super::super::render`]), each of which
//! adds its own `impl App` block. Because those blocks share `App`'s private
//! fields, the fields (and the private helper types/consts here) are
//! `pub(in crate::ui::app)` so every sibling submodule can reach them.

use std::sync::Arc;

use ratatui::layout::Rect;

use crate::ui::composer::Draft;
use crate::ui::theme::Theme;
use medulla::client::{FeedbackComment, FeedbackItem, FeedbackQuery, FeedbackType};
use medulla::config::LoadedConfig;
use medulla::runtime::{ContextItem, Runtime, RuntimeSnapshot, WorkerOp};

use super::picker::*;
use super::rail_hit::RailHit;

/// The ordered top-level tab names. The tab index selects into this array.
///
/// Trace and Context used to live here. They are secondary surfaces —
/// two of them diagnostic — so they now sit under Settings, keeping the tab bar
/// to the views a session is actually driven from.
///
/// Chat used to live here too, and is now the Sessions tab: talking to the
/// orchestrator *is* selecting its lane and typing. Splitting them meant reading
/// what an operation was doing on one tab and steering it on another, with two
/// scroll positions and no way to answer an agent's question from where the
/// question was visible.
///
/// Workflows used to be a Routing subpage. It is a tab because it is not a
/// management surface: Routing is where an operator declares what capacity
/// exists, and a workflow is *work* — a plan they read, edit, and run, with a
/// graph to navigate and a copilot to edit it by. Three panes' worth of surface
/// does not fit in a subpage of something else.
/// Subconscious sits after Workflows because it is the tier *under* the work
/// rather than another view of it: what gets filtered on the way in, what is
/// learned from the difference between expectation and outcome, and what is
/// escalated for a human to approve. It is a tab and not a Settings subpage for
/// the same reason Workflows is — approvals are work an operator acts on, not a
/// setting they configure. It draws a placeholder for now, and is listed anyway,
/// because an operator who can see where approvals will surface knows the layer
/// is not going to act behind their back.
///
/// `Tasks` and `Memory` are commented out rather than deleted: the code behind
/// both still builds and their render paths are intact, so restoring either is
/// putting one line back. Memory is out of the build entirely (its tab said
/// "coming soon"); Tasks duplicates what the Sessions tab already shows per lane.
///
/// `Changes` is gone rather than commented out: a Git diff is a property of one
/// session — what it changed since it launched — not a view over the whole fleet.
/// It lives on the Sessions tab as the `d` pane (`PaneView::Diff`), drawn over
/// the harness terminal for the row under the cursor. The shared diff state and
/// bindings stay in `app::changes`; only the top-level tab and its `D` shortcut
/// were removed.
#[cfg(feature = "workflows")]
pub const TABS: [&str; 7] = [
    "Overview",
    "Sessions",
    "Workflows",
    "Subconscious",
    "Hosts",
    "Feedback",
    "Settings",
];

/// Without the workflow engine. A slim build must not offer a tab that cannot
/// draw anything.
#[cfg(not(feature = "workflows"))]
pub const TABS: [&str; 6] = [
    "Overview",
    "Sessions",
    "Subconscious",
    "Hosts",
    "Feedback",
    "Settings",
];

/// The Routing tab's left-nav pages.
///
/// Ordered by the containment chain. `Hosts` is the machine level the operator
/// registers and steers by hand; `Harness Types` is the runtime level, which is
/// where credentials live — a subscription or an API key is a property of the
/// CLI runtime that spends it, not of the machine it happens to sit on;
/// `Workspaces` is the folder level, which is what the orchestrator actually
/// reasons about — a machine is capacity, a directory is *work*; `Agent
/// Templates` is the catalog of what may be provisioned onto any of it. `Add
/// Host` and `Strategies` are the two actions that belong to no level.
///
/// There is no `Fleet` page: the whole declared tree lives in the Sessions rail,
/// beside the lanes running on it. These pages are the *management* surfaces —
/// what you register, authenticate, and choose — not the picture. Workflows is
/// not here either: it is a tab of its own (see [`TABS`]).
/// Ordered by the containment chain, as before: the machine, what runs on it,
/// what may be stood up there, how to add another, and how work is routed
/// between them.
///
/// Only Workspaces is commented out. An entry there was advisory routing
/// context; declaring an agent is what actually puts work in a directory, and
/// that is done from the host tree. Its draw arm, keys and `[host].workspaces`
/// persistence all still build, so restoring it is putting its name back here
/// and renumbering.
pub const ROUTING_SUBPAGES: [&str; 6] = [
    "Hosts",
    "Harness Types",
    "Hooks",
    "Agent Templates",
    "Add Host",
    "Strategies",
];

pub(in crate::ui::app) const RP_HOSTS: usize = 0;
pub(in crate::ui::app) const RP_HARNESSES: usize = 1;
// Beside Harness Types on purpose: a hook is a property of every harness
// Medulla launches, and this page is the one place they are declared for all of
// them.
pub(in crate::ui::app) const RP_HOOKS: usize = 2;
pub(in crate::ui::app) const RP_TEMPLATES: usize = 3;
pub(in crate::ui::app) const RP_ADD_HOST: usize = 4;
pub(in crate::ui::app) const RP_STRATEGIES: usize = 5;
// Past the end of `ROUTING_SUBPAGES`, so the nav clamp cannot reach it and its
// arm is unreachable — the page is off without its code rotting.
pub(in crate::ui::app) const RP_WORKSPACES: usize = 6;

/// The TokenMaxxxing tab's sidebar pages.
pub(in crate::ui::app) const TOKENMAXXING_SUBPAGES: [&str; 3] =
    ["Overview", "Bounties", "Leaderboard"];

pub(in crate::ui::app) const TM_OVERVIEW: usize = 0;
pub(in crate::ui::app) const TM_BOUNTIES: usize = 1;
pub(in crate::ui::app) const TM_LEADERBOARD: usize = 2;

pub(in crate::ui::app) use super::super::routing_options::{
    ROUTING_STRATEGIES, SUBSCRIPTION_STRATEGIES,
};

/// The Settings tab's left-nav subpages, in order (number keys 1-9 jump to them).
///
/// This is the flat, selectable list [`App::settings_index`] indexes into.
/// [`SETTINGS_GROUPS`] overlays the display-only headings.
pub const SETTINGS_SUBPAGES: [&str; 9] = [
    "Usage",
    "Appearance",
    "Status line",
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
pub(in crate::ui::app) const SP_USAGE: usize = 0;
pub(in crate::ui::app) const SP_APPEARANCE: usize = 1;
pub(in crate::ui::app) const SP_STATUS_LINE: usize = 2;
pub(in crate::ui::app) const SP_CONFIG: usize = 3;
pub(in crate::ui::app) const SP_FEEDBACK: usize = 4;
pub(in crate::ui::app) const SP_TRACE: usize = 5;
pub(in crate::ui::app) const SP_CONTEXT: usize = 6;
pub(in crate::ui::app) const SP_ACCOUNT: usize = 7;
pub(in crate::ui::app) const SP_HELP: usize = 8;

/// The index of a tab by name, or 0 if unknown. Keeps tab jumps robust as the tab
/// list grows.
pub(in crate::ui::app) fn tab_pos(name: &str) -> usize {
    TABS.iter().position(|t| *t == name).unwrap_or(0)
}

/// What the pane beside the rail is showing for the selected harness.
///
/// The harness screen is not the only thing worth looking at for a session, and
/// the alternatives are all *about* that session rather than beside it: what it
/// has changed, and — as more of them land — what it is running. So they take
/// the pane's real estate rather than opening somewhere else, the way a tab
/// switch replaces a page: one thing on screen, one key to swap it, and the
/// rail cursor never moves.
///
/// Scoped to the selected session and reset when the cursor moves off it
/// ([`App::resolve_selected_session`](crate::ui::app)): a view opened to answer
/// a question about one harness must not stay open over the next one, where it
/// would be showing another session's diff under this session's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneView {
    /// The harness's own terminal — what it is painting right now.
    #[default]
    Harness,
    /// What the harness has changed since it launched.
    Diff,
}

/// Which pane of the Workflows tab has the keyboard.
///
/// Focus is split by *mode*, the way Settings and Routing split theirs: the
/// sidebar picks what is being looked at and hands over with `Enter`, the canvas
/// walks the graph, and the copilot is a composer that takes every printable
/// key. `Esc` steps back out one level at a time.
///
/// `Tab` is not part of this. It belongs to the top-level view ring, and a tab
/// that cycled its own panes with it would be a tab inside a tab — `c` reaches
/// the copilot instead.
#[cfg(feature = "workflows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkflowFocus {
    /// The catalogue sidebar: arrows walk workflows and their runs.
    #[default]
    Sidebar,
    /// The graph canvas: arrows walk nodes along their edges.
    Canvas,
    /// The copilot composer: printable keys type, Enter sends.
    Copilot,
}

/// What the Workflows content pane is showing.
///
/// One view at a time, beside the catalogue sidebar — the same two-pane shape
/// as Routing and Settings. Derived from [`WorkflowFocus`] and the inspector
/// toggle by [`App::workflow_view`] rather than stored, so it cannot drift from
/// the state that decides it.
#[cfg(feature = "workflows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowView {
    /// The laid-out graph, with any selected run overlaid on it.
    Graph,
    /// The selected node's declaration, and how a run left it.
    Inspector,
    /// The conversation that edits the graph.
    Copilot,
}

/// Everything the Workflows tab holds that is not the catalogue itself.
///
/// Grouped into one struct rather than a dozen `workflow_*` fields on [`App`]:
/// the tab has three panes with their own cursors, and a flat namespace made it
/// impossible to see which cursor belonged to which pane.
#[cfg(feature = "workflows")]
#[derive(Debug, Default)]
pub struct WorkflowsState {
    /// Which pane has the keyboard.
    pub(in crate::ui::app) focus: WorkflowFocus,
    /// Whether the rail cursor is on the "New workflow" row.
    ///
    /// Its own flag rather than a sentinel value of the catalogue index,
    /// because the New row is not a workflow: it has no graph to draw, no runs
    /// to list, and nothing to run. Everything that reads the selection has to
    /// answer "or is it the new one?" and a magic index would let that question
    /// go unasked.
    pub(in crate::ui::app) creating: bool,
    /// The selected workflow's run, when the rail cursor is on one of the run
    /// rows nested under it rather than on the workflow itself.
    ///
    /// A tree cursor rather than an index into a flattened row list: the rows
    /// under a workflow only exist while it is selected, so a flat index would
    /// have to be reinterpreted every time the cursor crossed a workflow
    /// boundary — and gets it wrong the moment a run appears mid-scroll.
    pub(in crate::ui::app) run_index: Option<usize>,
    /// The selected workflow's graph, as last read from the store. Cached
    /// because a render pass must not touch the disk, and re-laying it out every
    /// frame would move boxes under the cursor.
    pub(in crate::ui::app) graph: Option<Box<medulla::workflows::WorkflowGraph>>,
    /// The selected workflow's own choice of harness and model, cached with the
    /// graph and for the same reason. Not part of the graph, so a preview
    /// reading only [`graph`](Self::graph) would report the host's harness for a
    /// workflow that pinned its own.
    pub(in crate::ui::app) defaults: medulla::workflows::WorkflowDefaults,
    /// The laid-out form of [`graph`](Self::graph).
    pub(in crate::ui::app) layout: medulla::ui::workflows::GraphLayout,
    /// Selected node in the canvas, in the layout's reading order.
    pub(in crate::ui::app) node_index: usize,
    /// Vertical scroll of the canvas, in rows.
    ///
    /// The only scroll the canvas has: the graph folds onto a new band whenever
    /// a layer would run past the right edge, so it is never wider than the
    /// pane and there is nothing to scroll horizontally. Counted in rows rather
    /// than lanes because a fold puts a band boundary between two lanes, and a
    /// scroll measured in lanes cannot address the gap.
    pub(in crate::ui::app) canvas_row: usize,
    /// Rows inside the graph panel during its most recent render.
    ///
    /// Navigation uses this measured viewport rather than the full terminal
    /// height, because the selected-node preview shares the content column.
    pub(in crate::ui::app) graph_rows: usize,
    /// Top line of the rich selected-step preview.
    pub(in crate::ui::app) preview_scroll: usize,
    /// Whether the inspector below the canvas is expanded over it.
    pub(in crate::ui::app) inspector_open: bool,
    /// The run being overlaid on the graph, when a run row is selected.
    pub(in crate::ui::app) overlay: Option<medulla::workflows::RunId>,
    /// The Sessions-rail run this state was last pointed at, so the mirror is
    /// re-established only when the rail cursor actually moves.
    ///
    /// The Sessions tab draws the workflow canvas inline for a selected run, which
    /// means every frame would otherwise call
    /// [`select_workflow`](crate::ui::app::App::select_workflow) — and that
    /// re-reads the run store and re-lays out the graph, both off the disk, at
    /// the app's full draw rate. Remembering the id turns that into one read per
    /// cursor move. `None` while the Agents cursor is not on a run, so stepping
    /// off a run and back onto it re-syncs rather than trusting a stale graph.
    pub(in crate::ui::app) mirrored_run: Option<String>,
    /// The newest report included in [`mirrored_run`](Self::mirrored_run).
    ///
    /// A live run keeps its identity while its current graph node and durable
    /// record change. Remembering the report generation lets the mirror avoid
    /// disk work on ordinary redraws without freezing the canvas at its first
    /// observed node.
    pub(in crate::ui::app) mirrored_run_updated_at: Option<i64>,
    /// One copilot thread per workflow, so switching in the rail does not show
    /// the previous workflow's conversation or lose this one's.
    pub(in crate::ui::app) copilots:
        std::collections::HashMap<String, medulla::ui::workflows::CopilotState>,
    /// The copilot composer's draft.
    pub(in crate::ui::app) draft: Draft,
    /// Scroll offset in the copilot transcript, in lines from the bottom.
    pub(in crate::ui::app) copilot_scroll: usize,
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
    /// Clear the session this host is signed in with.
    Logout,
    /// Apply a worker fleet mutation.
    WorkerOp(WorkerOp),
    /// Apply several fleet mutations as one operator action.
    ///
    /// Removing a *host* is the case this exists for: a host is a group of
    /// roster entries sharing an address, and the registry has no host-level
    /// op — so taking one out means taking each of its agents out. Carrying
    /// them together keeps that one keypress one status line, rather than N
    /// racing "Worker registry updated" messages for what the operator did
    /// once. They are applied in order, and a failure reports the op it
    /// stopped on instead of being swallowed by the next success.
    WorkerOps(Vec<WorkerOp>),
    /// Retarget the live screen subscription: stop watching one task, start
    /// watching another. Both halves ride one command so the change is atomic
    /// from the loop's point of view — a stop that landed without its start
    /// would leave the pane blank with nothing on the way.
    WatchTask {
        /// The `(worker address, task id)` to stop streaming, if any.
        stop: Option<(String, String)>,
        /// The `(worker address, task id)` to start streaming, if any.
        start: Option<(String, String)>,
    },
    /// Kill the session serving a watched task after UI confirmation.
    KillTask {
        /// The worker address that owns the session.
        worker: String,
        /// The dispatched task whose session should be killed.
        task_id: String,
    },
    /// Push a handoff brief for a session the operator just gave back.
    ///
    /// Off the render thread because it does two things that must not block a
    /// frame: shells out to `git` for the branch, and awaits a socket emit.
    /// Arrives with `branch`/`project` unset — the dispatcher fills them.
    HandOffSession(Box<medulla::hub::HarnessHandoff>),
    /// Tell the orchestrator the operator has taken the session in a workspace.
    HoldSession {
        /// The workspace being taken.
        workspace: String,
        /// Why, when the operator said.
        reason: Option<String>,
    },
    /// Fetch account-level usage from the backend for the Usage tab.
    LoadUsage,
    /// Load a page of the feedback board for the Feedback surface.
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
    /// Re-read the declared fleet (roster + capacity) from the runtime.
    RefreshFleet,
    /// Run an installed workflow on this machine.
    ///
    /// Off-thread like every other filesystem/process command: a workflow run
    /// dispatches real agent sessions and takes minutes, so doing it on the
    /// render thread would freeze the app for the whole run.
    #[cfg(feature = "workflows")]
    RunWorkflow {
        /// The workflow to run.
        id: String,
        /// Values for the workflow's declared inputs, collected from the
        /// operator before this command was emitted. Empty when the workflow
        /// declares none.
        inputs: serde_json::Map<String, serde_json::Value>,
    },
    /// Ask the copilot to change or explain a workflow.
    ///
    /// Off-thread for the same reason a run is: the turn starts a real agent
    /// session, and the pane it reports into has to keep repainting while it
    /// does.
    #[cfg(feature = "workflows")]
    CopilotTurn {
        /// The workflow the turn is scoped to.
        workflow: String,
        /// The operator's instruction, verbatim.
        instruction: String,
    },
    /// Ask the copilot to build a workflow that does not exist yet.
    ///
    /// Separate from [`Cmd::CopilotTurn`] because it has no workflow to name:
    /// the agent is told to call `workflow_create`, and which workflow appeared
    /// is worked out from the store afterwards.
    #[cfg(feature = "workflows")]
    CreateWorkflow {
        /// Which copilot thread the turn's progress and result belong to.
        ///
        /// Carried rather than assumed: the thread for a workflow that does not
        /// exist is keyed by a sentinel the app owns, and an event loop that
        /// had to know that sentinel would be a second place it is spelled.
        thread: String,
        /// The operator's description of what they want, verbatim.
        instruction: String,
    },
    /// Simulate a workflow without dispatching anything, and report the result.
    #[cfg(feature = "workflows")]
    DryRunWorkflow {
        /// The workflow to simulate.
        id: String,
        /// Values for the workflow's declared inputs — a simulation resolves
        /// `=inputs.<name>` bindings like a real run, so it needs them too.
        inputs: serde_json::Map<String, serde_json::Value>,
    },
    /// Take back a workflow's most recent edit.
    ///
    /// Off-thread with the rest: it reads the history directory and writes a
    /// definition, and the store's methods are synchronous by contract.
    #[cfg(feature = "workflows")]
    UndoWorkflow {
        /// The workflow to restore.
        id: String,
    },
    /// Stop the copilot turn running on a thread.
    #[cfg(feature = "workflows")]
    AbortCopilot {
        /// Which copilot thread to stop.
        thread: String,
    },
    /// Ask the copilot to diagnose a failed run and fix its cause.
    ///
    /// Separate from [`Cmd::CopilotTurn`] because it carries the failure: the
    /// run, its error, and the nodes implicated. All three are on screen when
    /// the operator presses the key, and a turn that had to rediscover them
    /// would start a step behind.
    #[cfg(feature = "workflows")]
    RepairWorkflow {
        /// The workflow the run belongs to.
        workflow: String,
        /// The operator's words, if they typed any.
        instruction: String,
        /// The run to diagnose.
        run_id: String,
    },
    /// Review a workflow against its own history.
    ///
    /// Unlike [`Cmd::RepairWorkflow`], this turn may not edit: it records what
    /// it learns and proposes changes for the operator to accept. The two are
    /// separate commands rather than one with a flag because they are different
    /// asks — repair is "fix this now", review is "what should change".
    #[cfg(feature = "workflows")]
    EvolveWorkflow {
        /// The workflow to review.
        workflow: String,
        /// The failed run to lead with, when the review was triggered by one.
        run_id: Option<String>,
    },
    /// Apply a proposed change to the saved graph.
    #[cfg(feature = "workflows")]
    AcceptProposal {
        /// The workflow being changed, so the pane can be refreshed.
        workflow: String,
        /// The proposal to apply.
        proposal_id: String,
    },
    /// Turn a proposed change down.
    #[cfg(feature = "workflows")]
    RejectProposal {
        /// The workflow the proposal was for.
        workflow: String,
        /// The proposal to decline.
        proposal_id: String,
        /// Why, recorded as a note so a later review does not propose it again.
        reason: String,
    },
}

/// The Feedback surface's state: the loaded page, the selected row, that row's
/// comments, and the active query.
pub(in crate::ui::app) struct FeedbackState {
    /// The current page of board items.
    pub(in crate::ui::app) items: Vec<FeedbackItem>,
    /// Total items matching the query across all pages.
    pub(in crate::ui::app) total: i64,
    /// The highlighted row.
    pub(in crate::ui::app) index: usize,
    /// Comments for [`FeedbackState::detail_id`], loaded lazily on selection.
    pub(in crate::ui::app) comments: Vec<FeedbackComment>,
    /// Which item [`FeedbackState::comments`] belongs to.
    pub(in crate::ui::app) detail_id: Option<String>,
    /// Scroll offset within the detail pane.
    pub(in crate::ui::app) detail_scroll: usize,
    /// The active filter/sort/pagination.
    pub(in crate::ui::app) query: FeedbackQuery,
    /// Whether the runtime serves a board at all. `false` renders a sign-in
    /// hint instead of an empty list.
    pub(in crate::ui::app) supported: bool,
    /// Whether a board load is in flight (drives the header's "loading…").
    pub(in crate::ui::app) loading: bool,
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

/// Cached credential-presence flags displayed by Routing's Manage Keys pane.
#[derive(Default)]
pub(in crate::ui::app) struct CredentialStatus {
    pub(in crate::ui::app) claude_subscription: bool,
    pub(in crate::ui::app) codex_subscription: bool,
    pub(in crate::ui::app) anthropic_api_key: bool,
    pub(in crate::ui::app) openai_api_key: bool,
    pub(in crate::ui::app) openrouter_api_key: bool,
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
    /// Git changes from the selected session or operator-chosen commit.
    pub(in crate::ui::app) changes: super::super::changes::GitChangesState,
    pub(in crate::ui::app) draft: Draft,
    pub(in crate::ui::app) selected: usize,
    /// The Overview tab's animated workflow graph. Held on the app because its
    /// simulation has to survive between frames; it is advanced by the draw
    /// path, which is the only thing that looks at it.
    pub(in crate::ui::app) graph: super::super::render::graph::Graph,
    pub(in crate::ui::app) status: String,
    /// A persistent "update vX.Y.Z available" banner, set by the background
    /// update checker; shown in the header until the app exits.
    pub(in crate::ui::app) update_notice: Option<String>,
    pub(in crate::ui::app) contexts: Vec<ContextItem>,
    pub(in crate::ui::app) context_index: usize,
    pub(in crate::ui::app) rail_index: usize,
    /// Which selectable rail row remains selected while the live rail is rebuilt.
    pub(in crate::ui::app) agent_anchor: Option<super::super::rail::RailAnchor>,
    /// Extra pages of sublanes revealed under an agent lane, keyed by lane key.
    ///
    /// Keyed by [`AgentLane::key`](crate::ui::agents::AgentLane::key) rather than
    /// by the lane's rail position, because lanes are re-folded from events every
    /// tick and a lane that appears or ends shifts every index below it — an
    /// expansion tied to a position would silently jump to a different agent.
    /// Absent means the lane shows its first page, which is the default every
    /// lane starts at.
    pub(in crate::ui::app) subtask_pages: std::collections::HashMap<String, usize>,
    /// The `(worker address, task id)` whose screen is currently subscribed.
    ///
    /// Held so a selection change can stop the old stream as well as start the
    /// new one: a subscription nobody is looking at costs the worker a sample,
    /// a ratchet advance and a send on every tick.
    pub(in crate::ui::app) watching: Option<(String, String)>,
    /// The watched `(worker, task)` awaiting destructive-action confirmation.
    pub(in crate::ui::app) kill_armed: Option<(String, String)>,
    pub(in crate::ui::app) agent_scroll: usize,
    /// Selected row in the command peek, while it is open.
    pub(in crate::ui::app) command_index: usize,
    /// Selected row on the Routing Hosts page.
    pub(in crate::ui::app) host_index: usize,
    /// Whether ↑↓ on the Hosts page drives the role toggles in the preview
    /// rather than the host list above it. Tab moves between the two.
    pub(in crate::ui::app) host_roles_focus: bool,
    /// Selected role in the preview's toggle list, while it has focus.
    pub(in crate::ui::app) host_role_index: usize,
    /// Selected row on the Routing Workspaces page.
    pub(in crate::ui::app) workspace_index: usize,
    /// Selected row on the Routing Agent Templates page.
    pub(in crate::ui::app) template_index: usize,
    /// OpenRouter-backed harness presets loaded from the active config.
    pub(in crate::ui::app) custom_harnesses: Vec<medulla::config::CustomHarnessConfig>,
    /// Selected row on the Routing Harness Types page.
    pub(in crate::ui::app) custom_harness_index: usize,
    /// Selected row on the Routing Hooks page.
    pub(in crate::ui::app) hook_index: usize,
    /// Lifecycle reports arriving from the harnesses this Medulla launched.
    ///
    /// Written by the control socket's `hook.report` handler and read here; an
    /// app with no control plane bound simply renders an empty log.
    pub(in crate::ui::app) hook_log: medulla::harness_hooks::HookEventLog,
    /// Scroll offset inside the open agent-template popup.
    pub(in crate::ui::app) template_scroll: usize,
    /// Whether the agent-template popup is open over the catalog.
    pub(in crate::ui::app) template_modal: bool,
    /// Selected row on the Routing Workflows page.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_index: usize,
    /// The installed workflows, as last read from disk.
    ///
    /// Cached rather than re-read every frame: the store is files, and a render
    /// pass should not do I/O. `r` re-reads it, as it does for templates.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflows: Vec<medulla::workflows::WorkflowSummary>,
    /// The selected workflow's runs, read when the selection changes rather
    /// than on every frame.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_runs: Vec<medulla::workflows::RunRecord>,
    /// Why the run history could not be read, if it could not.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_runs_error: Option<String>,
    /// What the selected workflow has learned, newest first.
    ///
    /// Cached beside the runs and refreshed with them, for the same reason: a
    /// render pass must not touch the disk.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_notes: Vec<medulla::workflows::WorkflowNote>,
    /// Changes proposed for the selected workflow, newest first.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_proposals: Vec<medulla::workflows::WorkflowProposal>,
    /// The Workflows tab's panes, cursors, and copilot threads.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) wf: WorkflowsState,
    /// A workflow store attached directly, overriding the layered one this
    /// client would otherwise resolve.
    ///
    /// The layered store always reads the current directory's
    /// `.medulla/workflows` as repository defaults, then overlays the
    /// user-global workflow directory. That is useful in a real session and
    /// wrong under test, where it makes the catalogue depend on the developer's
    /// checkout. `None` resolves the layered store, as a real session does.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) workflow_store_override:
        Option<Arc<dyn medulla::workflows::WorkflowStore>>,
    /// The active Routing subpage (index into [`ROUTING_SUBPAGES`]).
    pub(in crate::ui::app) routing_index: usize,
    /// Whether keyboard focus is inside the Routing content pane.
    pub(in crate::ui::app) routing_focused: bool,
    /// Selected row on the Routing strategy page.
    pub(in crate::ui::app) routing_strategy_index: usize,
    /// Selected subscription rule on the Routing strategy page.
    pub(in crate::ui::app) subscription_strategy_index: usize,
    /// Whether the subscription group, rather than the host group, has focus.
    pub(in crate::ui::app) subscription_strategy_focused: bool,
    /// Credential presence captured on startup and refreshed when its pane opens.
    pub(in crate::ui::app) credential_status: CredentialStatus,
    /// The active TokenMaxxxing sidebar page.
    pub(in crate::ui::app) tokenmaxxing_index: usize,
    /// Whether keyboard focus is inside the TokenMaxxxing content pane.
    pub(in crate::ui::app) tokenmaxxing_focused: bool,
    /// Feedback-board state (lazily loaded on entry / refresh).
    pub(in crate::ui::app) feedback: FeedbackState,
    /// Feedback-board tab state (lazily loaded on tab entry / refresh).
    /// Whether the prepared-decision modal is visible.
    pub(in crate::ui::app) decision_open: bool,
    /// Highlighted decision row.
    pub(in crate::ui::app) decision_index: usize,
    /// Session-local ids intentionally hidden by the operator.
    pub(in crate::ui::app) dismissed_decisions: std::collections::BTreeSet<String>,
    pub(in crate::ui::app) prompt: Option<Prompt>,
    /// The animation frame counter: one per event-loop tick (~90ms).
    ///
    /// Drives the spinner and the workflow canvas's flowing wires. Held on the
    /// app rather than read from a clock so a test that draws frames explicitly
    /// sees the same animation the terminal does.
    pub frame: usize,
    /// Whether the app currently captures the mouse.
    pub mouse_capture: bool,
    /// Account-level usage payload (`/teams/me/usage` data), when fetched.
    pub account_usage: Option<serde_json::Value>,
    /// The active Settings subpage (index into [`SETTINGS_SUBPAGES`]).
    pub(in crate::ui::app) settings_index: usize,
    /// Whether keyboard focus is inside the Settings content pane rather than on
    /// the left-hand subpage nav.
    ///
    /// Subpages whose content is a list of *actions* (Feedback especially) bind
    /// enough single letters that they swallow the keys you would otherwise use
    /// to get around, and `↑↓` moving the nav meant arrow keys jumped you off
    /// the page entirely. Entering the pane hands `↑↓` to the content and makes
    /// the letter bindings deliberate rather than ambient.
    pub(in crate::ui::app) settings_focused: bool,
    /// The selected theme role on the Appearance subpage.
    pub(in crate::ui::app) appearance_index: usize,
    /// Throttled sampler backing the optional local-process status indicators.
    pub(in crate::ui::app) resource_monitor: crate::ui::resources::ResourceMonitor,
    /// Throttled sampler backing the optional whole-device sidebar indicators.
    pub(in crate::ui::app) device_monitor: crate::ui::resources::DeviceMonitor,
    /// The selected field row on the Status line subpage.
    pub(in crate::ui::app) status_line_index: usize,
    /// Whether the next persisted status-line edit must write the complete
    /// legacy-derived section rather than one field.
    pub(in crate::ui::app) status_line_promotion_pending: bool,
    /// The selected editable row on the Config subpage.
    pub(in crate::ui::app) config_index: usize,
    /// Whether the Account subpage's logout is armed. Logging out clears stored
    /// credentials, so the first Enter arms and the second confirms; any other
    /// navigation disarms it.
    pub(in crate::ui::app) logout_armed: bool,
    /// Whether the app is quitting in order to re-authenticate rather than to
    /// exit. Set by a successful logout so the caller tears the session down and
    /// returns to the login screen instead of returning to the shell.
    pub(in crate::ui::app) relogin_requested: bool,
    /// Who the embedded core is signed in as, for the Account subpage.
    pub(in crate::ui::app) account: Option<medulla::core_host::auth::AuthState>,
    /// The Medulla home directory, used to locate the credential store the
    /// Account subpage clears. Injectable so feature tests never touch the real
    /// home; `None` disables logout.
    pub(in crate::ui::app) medulla_home: Option<std::path::PathBuf>,
    /// The resolved color theme; selection highlighting + chrome draw from it.
    pub(in crate::ui::app) theme: Theme,
    /// Where appearance changes are persisted (the user-global `config.toml`).
    /// Injectable so feature tests never touch the real home. `None` disables
    /// persistence (changes still apply live).
    pub(in crate::ui::app) config_path: Option<std::path::PathBuf>,
    /// Where hook edits are persisted — deliberately not always [`Self::config_path`].
    ///
    /// `config_path` may resolve to a project-local file
    /// (`.medulla/config.toml`/`medulla.toml`), which is exactly the layer
    /// `medulla::config::load_config` strips `[[hooks]]` from on every load that
    /// is not an explicit `--config` (project configuration must not authorize
    /// shell commands in the operator's environment). Saving a hook there would
    /// show "Hook saved" and apply for the rest of this session while writing
    /// to a file the next launch ignores. Defaulted to [`Self::config_path`] by
    /// [`Self::set_config_path`] and overridden by
    /// [`Self::set_hooks_config_path`] whenever the caller knows the two must
    /// differ — see `app_loop::run_tui` in the `medulla-tui` crate.
    pub(in crate::ui::app) hooks_config_path: Option<std::path::PathBuf>,
    pub(in crate::ui::app) resume_picker: Option<ResumePicker>,
    /// Whether the event loop should exit after this tick.
    pub should_quit: bool,

    // Render geometry, recorded each draw for click hit-testing.
    pub(in crate::ui::app) area: Rect,
    pub(in crate::ui::app) hit_tabs: Vec<(u16, u16)>,
    pub(in crate::ui::app) hit_tabs_row: u16,
    /// Where the Sessions rail drew, and the rendered row each visible line
    /// belongs to. A row may wrap onto several lines, so a click resolves
    /// through this snapshot rather than by adding an offset to a freshly
    /// rebuilt row list that may have changed since the frame was drawn.
    pub(in crate::ui::app) hit_agents: Option<(Rect, Vec<RailHit>)>,
    // Where the embedded session screen landed, and whose it is. Recorded so a
    // wheel event can be routed to the terminal under the pointer and given
    // coordinates relative to *its* origin rather than the screen's.
    pub(in crate::ui::app) hit_session: Option<(Rect, String)>,
    /// The threads strip's hit box and its first visible row, for click-to-switch.
    pub(in crate::ui::app) hit_threads: Option<(Rect, usize)>,
    /// Where the orchestrator's conversation drew, and the task each of its
    /// visible lines opens (§A7) — `None` for the lines that are transcript
    /// rather than a session entry.
    ///
    /// One slot per drawn row rather than a dense list, because the entries are
    /// interleaved with the conversation: each one sits under the turn that
    /// started it, so the block is no longer contiguous and an offset from its
    /// top no longer identifies an entry.
    ///
    /// Tasks rather than row indices: the rail is rebuilt every frame, so an
    /// index recorded during the draw can name a different row by the time the
    /// click lands. A task id either still has a session or does not.
    pub(in crate::ui::app) hit_started_sessions: Option<(Rect, Vec<Option<String>>)>,
    pub(in crate::ui::app) hit_context: Option<Rect>,
    /// The selected workflow step's preview, for pointer-wheel scrolling.
    pub(in crate::ui::app) hit_workflow_preview: Option<Rect>,
    /// Where the active tab's subpage nav drew its page rows. Only one nav is on
    /// screen at a time, so one field serves Routing and Settings.
    pub(in crate::ui::app) hit_nav: crate::ui::multi_pane::NavHits,
    /// Every pane drawn this frame, in draw order. A pointer selection is
    /// clamped to whichever of these it started in, so a drag reads one pane's
    /// text instead of splicing its neighbour's columns into every row.
    pub(in crate::ui::app) panes: Vec<Rect>,
    /// A drag in progress: where the button went down. Kept apart from
    /// [`Self::selection`] so a click that never moves leaves no selection.
    pub(in crate::ui::app) drag_anchor: Option<(u16, u16)>,
    /// The block of cells the pointer has swept, normalized to
    /// `(left, top, right, bottom)` inclusive.
    pub(in crate::ui::app) selection: Option<(u16, u16, u16, u16)>,
    /// Set when the button is released over a live selection: the next draw
    /// copies what the selection covers, since only then is the buffer readable.
    pub(in crate::ui::app) copy_selection: bool,
    pub(in crate::ui::app) last_events_len: usize,

    // Test-only clipboard capture: when set, `copy_chat` records the copied text
    // here and skips the platform writers (no `pbcopy`/OSC subprocess in tests).
    pub(in crate::ui::app) copy_capture: Option<Arc<std::sync::Mutex<Vec<String>>>>,

    // Optional observational overlay from the background host-link service:
    // this endpoint's own identity, its peer roster, and peer presence. Merged
    // into the snapshot on every refresh so the Overview panel and Agents lanes
    // light up without the runtime having to know about the link.
    pub(in crate::ui::app) link_obs:
        Option<Arc<std::sync::Mutex<medulla::protocol::service::LinkObservation>>>,
    // A read-only view of the task host running on this device, when one is.
    // Read live at render rather than merged into the snapshot: its counters
    // move on the host's own schedule, and the snapshot is the *runtime's*
    // picture of the world — the host is a peer to it, not part of it.
    pub(in crate::ui::app) host_obs: Option<medulla::daemon::embedded::HostObservation>,
    // The live sessions this device is running. `None` when this machine
    // does not host, in which case the Sessions tab has no local screen to show
    // and falls back to a remote worker's streamed one, or to the transcript.
    pub(in crate::ui::app) local_sessions: Option<crate::ui::harness_pane::LocalSessions>,
    // Workflow runs the harnesses on this device started over MCP, keyed by the
    // grant session the launcher recorded on each PTY row. Read at render so a
    // run reported a moment ago is on screen at the next frame, and empty on a
    // build or a host with no control plane bound.
    pub(in crate::ui::app) harness_runs: medulla::control_socket::HarnessRunRegistry,
    // Live per-node harness output for runs this TUI started, keyed by run id.
    // Only ever a handful: a settled run keeps its frames until the next run of
    // the same workflow replaces it.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) live_runs:
        std::collections::HashMap<String, super::super::workflows::LiveRun>,
    // Which of the TUI and the selected session owns the keyboard. Reset to
    // `Chrome` whenever the attached session stops being the selected one, so
    // the operator's keys can never land in a session they are not looking at.
    pub(in crate::ui::app) harness_focus: crate::ui::harness_pane::HarnessFocus,
    // The session the Sessions pane resolved on the last draw, and the
    // only one the attach chord can act on. Recorded during render because that
    // is where the rail cursor is turned into a selection; cleared at the top of
    // every draw so it can never name a pane that is no longer on screen.
    pub(in crate::ui::app) pane_session: Option<String>,
    // What the pane is showing for that session: its terminal, or one of the
    // views that replace it. Not cleared per frame — it is the operator's
    // choice, not a record of what the last draw resolved — so it is reset by
    // the selection moving instead.
    pub(in crate::ui::app) pane_view: PaneView,
    // The session `pane_view` was chosen for. `pane_session` cannot answer this:
    // it is cleared at the top of every draw, so comparing against it would
    // reset the view on every frame.
    pub(in crate::ui::app) pane_view_session: Option<String>,
    // The session whose close is awaiting confirmation, if one is. Killing a
    // harness ends whatever it was in the middle of, so `k` asks first — the
    // same one-keypress contract `kill_armed` has for a dispatched task.
    pub(in crate::ui::app) harness_close_armed: Option<String>,
    // The session that took the press of the button currently held down, if any.
    // A terminal grabs the pointer for the whole gesture: whoever received the
    // press receives the drags and the release too, wherever the pointer has
    // wandered to since. Without the grab a release outside the pane — or one
    // swallowed by a modal the click itself opened — never reaches the child,
    // which goes on believing the button is still down and misplaces everything
    // it draws in response to the pointer afterwards.
    pub(in crate::ui::app) pointer_grab: Option<PointerGrab>,
    // Where the hand-back question drew each of its answers, and the key each
    // one stands for. Recorded during the draw so a click can be answered by
    // replaying the keystroke rather than by a second copy of the routing: the
    // two would drift, and the direction they would drift in is a pointer that
    // hands a harness back when the operator meant to keep it.
    pub(in crate::ui::app) hit_handback: Vec<(Rect, crossterm::event::KeyCode)>,
    // The "start a session" picker's outer box, and where each offered row was
    // drawn with the index it stands for in that step's list. Recorded during
    // the draw for the same reason the hand-back answers are: the harness step
    // windows a long list, so screen position and list index are not the same
    // number, and only the draw knows which window it used.
    pub(in crate::ui::app) hit_session_picker: Option<(Rect, Vec<(Rect, usize)>)>,
    // The agent behind a selected session row that this device does NOT run,
    // recorded alongside `pane_session` on the same draw.
    //
    // Its only purpose is to tell "the cursor is not on a session" apart from
    // "the cursor is on somebody else's session", which `pane_session` cannot:
    // both leave it `None`. Taking control resolves through the local workspace
    // path, so a remote session can be watched but not taken (§E7), and an
    // operator who presses the take chord on one deserves that answer rather
    // than "no session on this row".
    pub(in crate::ui::app) pane_remote_session: Option<String>,
    // The session selected on the Sessions rail, retained while another tab is
    // visible. Unlike `pane_session`, this is navigation state rather than a
    // keyboard-routing capability: Changes uses it to keep following
    // the repository the operator selected after an intervening tab draw.
    pub(in crate::ui::app) rail_session: Option<String>,
    /// The "start a session" picker, while it is open.
    pub(in crate::ui::app) session_picker: Option<SessionPicker>,
    /// The "you still hold this session" confirmation, while it is open.
    pub(in crate::ui::app) handback_prompt: Option<HandbackPrompt>,
    /// How far the Help page is scrolled, in lines.
    pub(in crate::ui::app) help_scroll: u16,
    /// What releasing a held session does, from `[harness].handback`.
    pub(in crate::ui::app) handback_policy: HandbackPolicy,
    /// The sessions taken *from the orchestrator*, and how each was taken.
    ///
    /// Membership is the whole question the release prompt exists to ask. A
    /// session the operator started themselves was never the orchestrator's, so
    /// letting go of the keyboard owes it nothing, and asking about it every
    /// time is how a confirmation becomes furniture. One taken out from under
    /// dispatch is different: walking away from it leaves the orchestrator
    /// locked out of a workspace, silently and indefinitely.
    ///
    /// The origin distinguishes "you picked this up by focusing in" from "you
    /// asked for it with /takecontrol", which the release prompt words
    /// differently: the second was a decision, and re-asking about it as though
    /// it were an accident is how a confirmation becomes noise.
    ///
    /// Keyed by session rather than kept as one flag because several sessions
    /// can be held at once — take A, keep it on release, then attach to B. A
    /// single flag answered for whichever was touched last: it worded B's
    /// question with A's takeover, and kept asking about sessions nobody had
    /// taken from anyone.
    pub(in crate::ui::app) sessions_taken: std::collections::HashMap<String, TakeOrigin>,
    /// Sessions the operator has at some point given to the orchestrator.
    ///
    /// Separate from [`sessions_taken`](Self::sessions_taken), which says who
    /// holds a session *now*; this remembers that dispatch once had a claim on
    /// it, and it is never cleared.
    ///
    /// Needed because [`SessionOrigin`](crate::worker::pty::SessionOrigin) alone
    /// under-counts. A session the operator started carries origin `User`
    /// forever, but handing it back makes it genuinely dispatchable —
    /// `SessionHandle::serves_label` lets a handed-back operator session be
    /// adopted for a task. If that turn then fails, the executor hands it
    /// straight back to the operator without going through
    /// [`take_session`](App::take_session), leaving a session with origin
    /// `User`, no entry in `sessions_taken`, and dispatch locked out of it.
    /// Releasing that in silence is the bug this set closes.
    pub(in crate::ui::app) orchestrator_claimed: std::collections::HashSet<String>,
    /// Commands raised by synchronous input handlers, drained by the event loop.
    ///
    /// The key and mouse handlers that move session control cannot return a
    /// [`Cmd`] — `handle_handback_key` returns `()`, `handle_harness_key`
    /// returns `bool`, and the mouse path returns nothing — and threading an
    /// `Option<Cmd>` back through all three would be a wide, test-breaking
    /// change to say one thing. So they push here instead, and the loop drains
    /// it right after the event that produced it. Commands run in submission
    /// order.
    pub(in crate::ui::app) pending_cmds: std::collections::VecDeque<Cmd>,
    /// Whether operator-started sessions launch with the permission-bypass
    /// flag, from `[harness].skipPermissions`.
    pub(in crate::ui::app) harness_skip_permissions: bool,
}
