//! The data model for the interactive TUI screen: the tab list, multi-pane
//! navigation constants, the [`Cmd`] the event loop runs on the app's behalf, the
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

use crate::ui::composer::{Draft, TextPrompt};
use crate::ui::theme::Theme;
use medulla::client::{FeedbackComment, FeedbackItem, FeedbackQuery, FeedbackType};
use medulla::config::LoadedConfig;
use medulla::memory::{MemoryHit, MemoryStatus};
use medulla::runtime::{ContextItem, Runtime, RuntimeSnapshot, WorkerOp};
use medulla::runtime::{RoutingStrategy, SubscriptionRoutingStrategy};

/// The ordered top-level tab names. The tab index selects into this array.
///
/// Trace, Context, and Feedback used to live here. They are secondary surfaces —
/// two of them diagnostic — so they now sit under Settings, keeping the tab bar
/// to the views a session is actually driven from.
///
/// Chat used to live here too, and is now the Agents tab: talking to the
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
#[cfg(feature = "workflows")]
pub const TABS: [&str; 8] = [
    "Overview",
    "Agents",
    "Tasks",
    "Workflows",
    "TokenMaxxxing",
    "Routing",
    "Memory",
    "Settings",
];

/// Without the workflow engine. A slim build must not offer a tab that cannot
/// draw anything.
#[cfg(not(feature = "workflows"))]
pub const TABS: [&str; 7] = [
    "Overview",
    "Agents",
    "Tasks",
    "TokenMaxxxing",
    "Routing",
    "Memory",
    "Settings",
];

/// The Routing tab's left-nav pages.
///
/// Ordered by the containment chain. `Hosts` is the machine level the operator
/// registers and steers by hand; `Harnesses` is the runtime level, which is
/// where credentials live — a subscription or an API key is a property of the
/// CLI runtime that spends it, not of the machine it happens to sit on;
/// `Workspaces` is the folder level, which is what the orchestrator actually
/// reasons about — a machine is capacity, a directory is *work*; `Agent
/// Templates` is the catalog of what may be provisioned onto any of it. `Add
/// Host` and `Strategies` are the two actions that belong to no level.
///
/// There is no `Fleet` page: the whole declared tree lives in the Agents rail,
/// beside the lanes running on it. These pages are the *management* surfaces —
/// what you register, authenticate, and choose — not the picture. Workflows is
/// not here either: it is a tab of its own (see [`TABS`]).
pub const ROUTING_SUBPAGES: [&str; 6] = [
    "Hosts",
    "Harnesses",
    "Workspaces",
    "Agent Templates",
    "Add Host",
    "Strategies",
];

pub(super) const RP_HOSTS: usize = 0;
pub(super) const RP_HARNESSES: usize = 1;
pub(super) const RP_WORKSPACES: usize = 2;
pub(super) const RP_TEMPLATES: usize = 3;
pub(super) const RP_ADD_HOST: usize = 4;
pub(super) const RP_STRATEGIES: usize = 5;

/// The Tasks tab's left-nav pages.
pub const TASKS_SUBPAGES: [&str; 2] = ["All Tasks", "Sources"];

pub(super) const TP_TASKS: usize = 0;
pub(super) const TP_SOURCES: usize = 1;

/// The TokenMaxxxing tab's sidebar pages.
pub(super) const TOKENMAXXING_SUBPAGES: [&str; 3] = ["Overview", "Bounties", "Leaderboard"];

pub(super) const TM_OVERVIEW: usize = 0;
pub(super) const TM_BOUNTIES: usize = 1;
pub(super) const TM_LEADERBOARD: usize = 2;

/// The Memory tab's left-nav pages.
pub const MEMORY_SUBPAGES: [&str; 5] =
    ["Overview", "Directives", "Facets", "Search", "Maintenance"];

pub(super) const MP_OVERVIEW: usize = 0;
pub(super) const MP_DIRECTIVES: usize = 1;
pub(super) const MP_FACETS: usize = 2;
pub(super) const MP_SEARCH: usize = 3;
pub(super) const MP_MAINTENANCE: usize = 4;

/// Display metadata coupled to the routing strategy it applies.
#[derive(Clone, Copy)]
pub(super) struct RoutingStrategyOption {
    /// Runtime strategy sent when the option is applied.
    pub(super) strategy: RoutingStrategy,
    /// Short label rendered in the strategy chooser.
    pub(super) label: &'static str,
    /// Operator-facing explanation of the selection rule.
    pub(super) description: &'static str,
}

/// Routing strategy options in the order shown by the chooser.
pub(super) const ROUTING_STRATEGIES: [RoutingStrategyOption; 4] = [
    RoutingStrategyOption {
        strategy: RoutingStrategy::Manual,
        label: "Manual",
        description: "Keep the host explicitly selected on the Hosts page.",
    },
    RoutingStrategyOption {
        strategy: RoutingStrategy::Balanced,
        label: "Balanced",
        description: "Choose the most CPU cores, breaking ties by available RAM.",
    },
    RoutingStrategyOption {
        strategy: RoutingStrategy::CpuFirst,
        label: "CPU First",
        description: "Choose the host with the most logical CPU cores.",
    },
    RoutingStrategyOption {
        strategy: RoutingStrategy::MemoryFirst,
        label: "Memory First",
        description: "Choose the host with the most currently available RAM.",
    },
];

/// Display metadata for one subscription-level selection rule.
#[derive(Clone, Copy)]
pub(super) struct SubscriptionStrategyOption {
    /// Runtime strategy sent when the option is applied.
    pub(super) strategy: SubscriptionRoutingStrategy,
    /// Short label rendered in the strategy chooser.
    pub(super) label: &'static str,
    /// Operator-facing explanation of the budget comparison.
    pub(super) description: &'static str,
}

/// Subscription strategy options in the order shown by the chooser.
pub(super) const SUBSCRIPTION_STRATEGIES: [SubscriptionStrategyOption; 3] = [
    SubscriptionStrategyOption {
        strategy: SubscriptionRoutingStrategy::Manual,
        label: "Manual",
        description: "Keep the requested provider or the host's configured default.",
    },
    SubscriptionStrategyOption {
        strategy: SubscriptionRoutingStrategy::Balanced,
        label: "Balanced",
        description: "Choose the ready subscription with the most remaining percentage.",
    },
    SubscriptionStrategyOption {
        strategy: SubscriptionRoutingStrategy::MostAvailableBudget,
        label: "Most Available Budget",
        description: "Choose the ready subscription with the most remaining tokens.",
    },
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

/// Which half of the Agents tab the keyboard is driving.
///
/// The tab merges a list (the rail) with a text input (the composer), and a
/// terminal has one keyboard for both. Typing has to work the instant the tab
/// opens — that is the point of folding chat in here — so the composer holds
/// focus by default and the bare arrows belong to the caret.
///
/// That left the rail reachable only by `Alt`+`↑`/`↓`, which most macOS
/// terminals do not send at all unless the user has rebound the Option key.
/// Focus is therefore explicit and movable, matching the menu/content model
/// Settings and Routing already use: `Esc` steps out to the rail, `Enter` (or
/// simply typing) steps back in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentsFocus {
    /// The composer has the keyboard: arrows move the caret, Enter submits.
    #[default]
    Composer,
    /// The rail has the keyboard: arrows walk the rows, Enter returns below.
    Rail,
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
    pub(super) focus: WorkflowFocus,
    /// Whether the rail cursor is on the "New workflow" row.
    ///
    /// Its own flag rather than a sentinel value of the catalogue index,
    /// because the New row is not a workflow: it has no graph to draw, no runs
    /// to list, and nothing to run. Everything that reads the selection has to
    /// answer "or is it the new one?" and a magic index would let that question
    /// go unasked.
    pub(super) creating: bool,
    /// The selected workflow's run, when the rail cursor is on one of the run
    /// rows nested under it rather than on the workflow itself.
    ///
    /// A tree cursor rather than an index into a flattened row list: the rows
    /// under a workflow only exist while it is selected, so a flat index would
    /// have to be reinterpreted every time the cursor crossed a workflow
    /// boundary — and gets it wrong the moment a run appears mid-scroll.
    pub(super) run_index: Option<usize>,
    /// The selected workflow's graph, as last read from the store. Cached
    /// because a render pass must not touch the disk, and re-laying it out every
    /// frame would move boxes under the cursor.
    pub(super) graph: Option<Box<medulla::workflows::WorkflowGraph>>,
    /// The laid-out form of [`graph`](Self::graph).
    pub(super) layout: medulla::ui::workflows::GraphLayout,
    /// Selected node in the canvas, in the layout's reading order.
    pub(super) node_index: usize,
    /// Horizontal scroll of the canvas, in layers.
    pub(super) canvas_layer: usize,
    /// Vertical scroll of the canvas, in lanes.
    pub(super) canvas_lane: usize,
    /// Whether the inspector below the canvas is expanded over it.
    pub(super) inspector_open: bool,
    /// The run being overlaid on the graph, when a run row is selected.
    pub(super) overlay: Option<medulla::workflows::RunId>,
    /// One copilot thread per workflow, so switching in the rail does not show
    /// the previous workflow's conversation or lose this one's.
    pub(super) copilots: std::collections::HashMap<String, medulla::ui::workflows::CopilotState>,
    /// The copilot composer's draft.
    pub(super) draft: Draft,
    /// Scroll offset in the copilot transcript, in lines from the bottom.
    pub(super) copilot_scroll: usize,
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
    /// Apply a worker fleet mutation.
    WorkerOp(WorkerOp),
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
    /// Reload the local task document.
    LoadTasks,
    /// Persist a new or edited local task.
    SaveTask(Box<medulla::tasks::Task>),
    /// Persist the complete local task document.
    SaveTasks(Box<medulla::tasks::TaskDocument>),
    /// Remove a local task by id.
    DeleteTask(String),
    /// Synchronize one configured task source.
    SyncTasks(String),
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
    /// Re-read the declared fleet (roster + capacity) from the runtime.
    RefreshFleet,
    /// Run an installed workflow on this machine.
    ///
    /// Off-thread like every other filesystem/process command: a workflow run
    /// dispatches real harness sessions and takes minutes, so doing it on the
    /// render thread would freeze the app for the whole run.
    #[cfg(feature = "workflows")]
    RunWorkflow {
        /// The workflow to run.
        id: String,
    },
    /// Ask the copilot to change or explain a workflow.
    ///
    /// Off-thread for the same reason a run is: the turn starts a real harness
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
#[derive(Clone)]
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

/// The action a small inline prompt (Hosts add/edit, Agents answer) submits.
pub(super) enum PromptKind {
    /// Create a task from a title line.
    TaskCreate,
    /// Edit the selected task title.
    TaskEdit(String),
    /// Add a GitHub source from `owner/repository`.
    SourceAdd,
    /// Search local persona memory with a natural-language query.
    MemorySearch,
    /// Add a worker from an address/@handle line.
    HostAdd,
    /// Edit the label of the worker with the given id.
    HostEditLabel(String),
    /// Declare another directory this device may work in.
    WorkspaceAdd,
    /// Answer a pending sub-agent question.
    AnswerQuestion {
        /// The cycle the question belongs to.
        cycle_id: String,
        /// The pending question's id.
        question_id: String,
    },
    /// Answer a prepared decision and dismiss it locally once routed.
    DecisionAnswer {
        /// Stable decision id.
        decision_id: String,
        /// Cycle that owns the question.
        cycle_id: String,
        /// Harness question id.
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

/// A single-line inline input overlay shared with daemon controls.
pub(super) type Prompt = TextPrompt<PromptKind>;

/// Cached credential-presence flags displayed by Routing's Manage Keys pane.
#[derive(Default)]
pub(super) struct CredentialStatus {
    pub(super) claude_subscription: bool,
    pub(super) codex_subscription: bool,
    pub(super) anthropic_api_key: bool,
    pub(super) openai_api_key: bool,
    pub(super) openrouter_api_key: bool,
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
    /// The `(worker address, task id)` whose screen is currently subscribed.
    ///
    /// Held so a selection change can stop the old stream as well as start the
    /// new one: a subscription nobody is looking at costs the worker a sample,
    /// a ratchet advance and a send on every tick.
    pub(super) watching: Option<(String, String)>,
    /// Which half of the Agents tab the keyboard is driving.
    pub(super) agents_focus: AgentsFocus,
    pub(super) agent_scroll: usize,
    pub(super) chat_scroll: usize,
    /// Selected row in the command peek, while it is open.
    pub(super) command_index: usize,
    /// Selected row on the Routing Hosts page.
    pub(super) host_index: usize,
    /// Selected row on the Routing Workspaces page.
    pub(super) workspace_index: usize,
    /// Selected row on the Routing Agent Templates page.
    pub(super) template_index: usize,
    /// Scroll offset inside the open agent-template popup.
    pub(super) template_scroll: usize,
    /// Whether the agent-template popup is open over the catalog.
    pub(super) template_modal: bool,
    /// Selected row on the Routing Workflows page.
    #[cfg(feature = "workflows")]
    pub(super) workflow_index: usize,
    /// The installed workflows, as last read from disk.
    ///
    /// Cached rather than re-read every frame: the store is files, and a render
    /// pass should not do I/O. `r` re-reads it, as it does for templates.
    #[cfg(feature = "workflows")]
    pub(super) workflows: Vec<medulla::workflows::WorkflowSummary>,
    /// The selected workflow's runs, read when the selection changes rather
    /// than on every frame.
    #[cfg(feature = "workflows")]
    pub(super) workflow_runs: Vec<medulla::workflows::RunRecord>,
    /// Why the run history could not be read, if it could not.
    #[cfg(feature = "workflows")]
    pub(super) workflow_runs_error: Option<String>,
    /// The Workflows tab's panes, cursors, and copilot threads.
    #[cfg(feature = "workflows")]
    pub(super) wf: WorkflowsState,
    /// A workflow store attached directly, overriding the layered one this
    /// client would otherwise resolve.
    ///
    /// The layered store always includes the *current directory's*
    /// `.medulla/workflows`, which is right in use — a workflow checked into the
    /// repository you are working in should be listed — and wrong under test,
    /// where it makes the catalogue depend on whatever happens to be in the
    /// developer's checkout. `None` resolves the layered store, as a real
    /// session does.
    #[cfg(feature = "workflows")]
    pub(super) workflow_store_override: Option<Arc<dyn medulla::workflows::WorkflowStore>>,
    /// The active Routing subpage (index into [`ROUTING_SUBPAGES`]).
    pub(super) routing_index: usize,
    /// Whether keyboard focus is inside the Routing content pane.
    pub(super) routing_focused: bool,
    /// Selected row on the Routing strategy page.
    pub(super) routing_strategy_index: usize,
    /// Selected subscription rule on the Routing strategy page.
    pub(super) subscription_strategy_index: usize,
    /// Whether the subscription group, rather than the host group, has focus.
    pub(super) subscription_strategy_focused: bool,
    /// Credential presence captured on startup and refreshed when its pane opens.
    pub(super) credential_status: CredentialStatus,
    /// The active Tasks subpage (index into [`TASKS_SUBPAGES`]).
    pub(super) tasks_index: usize,
    /// Whether keyboard focus is inside the Tasks content pane.
    pub(super) tasks_focused: bool,
    /// Selected provider row on the Tasks Sources page.
    pub(super) task_source_index: usize,
    /// Whether the selected task or source's detail modal is visible.
    pub(super) tasks_detail_open: bool,
    /// The active TokenMaxxxing sidebar page.
    pub(super) tokenmaxxing_index: usize,
    /// Whether keyboard focus is inside the TokenMaxxxing content pane.
    pub(super) tokenmaxxing_focused: bool,
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
    /// The active Memory subpage (index into [`MEMORY_SUBPAGES`]).
    pub(super) memory_subpage_index: usize,
    /// Whether keyboard focus is inside the Memory content pane.
    pub(super) memory_focused: bool,
    /// Whether the selected Memory entry's detail modal is visible.
    pub(super) memory_detail_open: bool,
    /// Feedback-board tab state (lazily loaded on tab entry / refresh).
    pub(super) feedback: FeedbackState,
    /// Durable local task document displayed by the Tasks tab.
    pub(super) tasks: medulla::tasks::TaskDocument,
    /// Whether the prepared-decision modal is visible.
    pub(super) decision_open: bool,
    /// Highlighted decision row.
    pub(super) decision_index: usize,
    /// Session-local ids intentionally hidden by the operator.
    pub(super) dismissed_decisions: std::collections::BTreeSet<String>,
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
    /// The threads strip's hit box and its first visible row, for click-to-switch.
    pub(super) hit_threads: Option<(Rect, usize)>,
    pub(super) hit_context: Option<Rect>,
    /// Where the active tab's subpage nav drew its page rows. Only one nav is on
    /// screen at a time, so one field serves Tasks, Routing, Memory, and
    /// Settings.
    pub(super) hit_nav: crate::ui::multi_pane::NavHits,
    /// Every pane drawn this frame, in draw order. A pointer selection is
    /// clamped to whichever of these it started in, so a drag reads one pane's
    /// text instead of splicing its neighbour's columns into every row.
    pub(super) panes: Vec<Rect>,
    /// A drag in progress: where the button went down. Kept apart from
    /// [`Self::selection`] so a click that never moves leaves no selection.
    pub(super) drag_anchor: Option<(u16, u16)>,
    /// The block of cells the pointer has swept, normalized to
    /// `(left, top, right, bottom)` inclusive.
    pub(super) selection: Option<(u16, u16, u16, u16)>,
    /// Set when the button is released over a live selection: the next draw
    /// copies what the selection covers, since only then is the buffer readable.
    pub(super) copy_selection: bool,
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
    // A read-only view of the task host running on this device, when one is.
    // Read live at render rather than merged into the snapshot: its counters
    // move on the host's own schedule, and the snapshot is the *runtime's*
    // picture of the world — the host is a peer to it, not part of it.
    pub(super) host_obs: Option<medulla::daemon::embedded::HostObservation>,
}
