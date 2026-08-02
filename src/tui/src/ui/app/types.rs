//! The data model for the interactive TUI screen: the tab list, multi-pane
//! navigation constants, the [`Cmd`] the event loop runs on the app's behalf, the
//! small overlay/state types ([`ResumePicker`], [`Prompt`], [`PromptKind`],
//! and the central [`App`] struct itself.
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
use medulla::runtime::{ContextItem, Runtime, RuntimeSnapshot, WorkerOp};

/// The ordered top-level tab names. The tab index selects into this array.
///
/// Trace and Context used to live here. They are secondary surfaces —
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
/// `Tasks` and `Memory` are commented out rather than deleted: the code behind
/// both still builds and their render paths are intact, so restoring either is
/// putting one line back. Memory is out of the build entirely (its tab said
/// "coming soon"); Tasks duplicates what the Agents tab already shows per lane.
#[cfg(feature = "workflows")]
pub const TABS: [&str; 6] = [
    "Overview",
    "Agents",
    "Workflows",
    "Hosts",
    "Feedback",
    "Settings",
];

/// Without the workflow engine. A slim build must not offer a tab that cannot
/// draw anything.
#[cfg(not(feature = "workflows"))]
pub const TABS: [&str; 5] = ["Overview", "Agents", "Hosts", "Feedback", "Settings"];

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
/// Ordered by the containment chain, as before: the machine, what runs on it,
/// what may be stood up there, how to add another, and how work is routed
/// between them.
///
/// Only Workspaces is commented out, and only because Add Host › Local
/// supersedes it: an entry there was advisory routing context, whereas a local
/// host actually runs work in its directory. Its draw arm, keys and
/// `[host].workspaces` persistence all still build, so restoring it is putting
/// its name back here and renumbering.
pub const ROUTING_SUBPAGES: [&str; 5] = [
    "Hosts",
    "Harnesses",
    "Agent Templates",
    "Add Host",
    "Strategies",
];

pub(super) const RP_HOSTS: usize = 0;
pub(super) const RP_HARNESSES: usize = 1;
pub(super) const RP_TEMPLATES: usize = 2;
pub(super) const RP_ADD_HOST: usize = 3;
pub(super) const RP_STRATEGIES: usize = 4;
// Past the end of `ROUTING_SUBPAGES`, so the nav clamp cannot reach it and its
// arm is unreachable — the page is off without its code rotting.
pub(super) const RP_WORKSPACES: usize = 5;

/// The TokenMaxxxing tab's sidebar pages.
pub(super) const TOKENMAXXING_SUBPAGES: [&str; 3] = ["Overview", "Bounties", "Leaderboard"];

pub(super) const TM_OVERVIEW: usize = 0;
pub(super) const TM_BOUNTIES: usize = 1;
pub(super) const TM_LEADERBOARD: usize = 2;

pub(super) use super::routing_options::{ROUTING_STRATEGIES, SUBSCRIPTION_STRATEGIES};

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
pub(super) const SP_USAGE: usize = 0;
pub(super) const SP_APPEARANCE: usize = 1;
pub(super) const SP_STATUS_LINE: usize = 2;
pub(super) const SP_CONFIG: usize = 3;
pub(super) const SP_FEEDBACK: usize = 4;
pub(super) const SP_TRACE: usize = 5;
pub(super) const SP_CONTEXT: usize = 6;
pub(super) const SP_ACCOUNT: usize = 7;
pub(super) const SP_HELP: usize = 8;

/// Which kind of host the Add Host page is collecting.
///
/// The two differ in everything that matters — a remote is reached by address
/// over tiny.place, a local one by a directory on this machine — so asking
/// which first is what lets each ask only for what it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddHostKind {
    /// A directory on this machine, served in-process.
    Local,
    /// Another machine, reached by its tiny.place address.
    Remote,
}

impl AddHostKind {
    /// The choices in the order they are offered.
    pub const ALL: [AddHostKind; 2] = [AddHostKind::Local, AddHostKind::Remote];

    /// The one-word name shown in the picker.
    pub fn label(self) -> &'static str {
        match self {
            AddHostKind::Local => "Local",
            AddHostKind::Remote => "Remote",
        }
    }

    /// What choosing this actually does, so the picker explains itself.
    pub fn description(self) -> &'static str {
        match self {
            AddHostKind::Local => {
                "a directory on this machine · runs in this process, watchable and typeable"
            }
            AddHostKind::Remote => {
                "another machine · reached by its tiny.place address, needs a contact edge"
            }
        }
    }
}

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
    /// The selected workflow's own choice of harness and model, cached with the
    /// graph and for the same reason. Not part of the graph, so a preview
    /// reading only [`graph`](Self::graph) would report the host's harness for a
    /// workflow that pinned its own.
    pub(super) defaults: medulla::workflows::WorkflowDefaults,
    /// The laid-out form of [`graph`](Self::graph).
    pub(super) layout: medulla::ui::workflows::GraphLayout,
    /// Selected node in the canvas, in the layout's reading order.
    pub(super) node_index: usize,
    /// Horizontal scroll of the canvas, in layers.
    pub(super) canvas_layer: usize,
    /// Vertical scroll of the canvas, in lanes.
    pub(super) canvas_lane: usize,
    /// Rows inside the graph panel during its most recent render.
    ///
    /// Navigation uses this measured viewport rather than the full terminal
    /// height, because the selected-node preview shares the content column.
    pub(super) graph_rows: usize,
    /// Top line of the rich selected-step preview.
    pub(super) preview_scroll: usize,
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
    /// Clear the session this host is signed in with.
    Logout,
    /// Apply a worker fleet mutation.
    WorkerOp(WorkerOp),
    /// Start a host on this device now, and register it with the hub.
    ///
    /// Carries the declaration rather than only an index into config: the
    /// config is the app's, and the loop that can actually start a host is not.
    /// `index` is the entry's position within `[[hosts]]`, which is the basis an
    /// unnamed host's address is derived from at every other site.
    StartLocalHost {
        /// The host declaration to bind.
        host: Box<medulla::config::HostSection>,
        /// Its position within `[[hosts]]`.
        index: usize,
    },
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
    /// Push a handoff brief for a harness the operator just gave back.
    ///
    /// Off the render thread because it does two things that must not block a
    /// frame: shells out to `git` for the branch, and awaits a socket emit.
    /// Arrives with `branch`/`project` unset — the dispatcher fills them.
    HandOffHarness(Box<medulla::hub::HarnessHandoff>),
    /// Tell the orchestrator the operator has taken the harness in a workspace.
    HoldHarness {
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
    /// dispatches real harness sessions and takes minutes, so doing it on the
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

/// The modal state for the "resume a chat" picker overlay.
pub(super) struct ResumePicker {
    /// The resumable chats to choose from.
    pub(super) chats: Vec<crate::ui::chat_store::MainChatSummary>,
    /// The highlighted row.
    pub(super) index: usize,
}

/// The modal state for the "start a harness" picker overlay.
pub(super) struct HarnessPicker {
    /// Installed providers and registered presets, in offer order.
    pub(super) choices: Vec<crate::ui::harness_pane::HarnessChoice>,
    /// The highlighted row.
    pub(super) index: usize,
    /// Which half of the two-step picker owns the keyboard.
    pub(super) step: HarnessPickerStep,
    /// Default directory used to seed the editable workspace query.
    pub(super) cwd: String,
    /// Inline fuzzy-completion text on the workspace step.
    pub(super) workspace_query: String,
    /// Cached workspace rows, refreshed only when the query changes.
    pub(super) workspace_choices: Vec<WorkspaceChoice>,
    /// Highlighted workspace completion.
    pub(super) workspace_index: usize,
}

/// Active stage of the manual harness launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HarnessPickerStep {
    /// Choose an installed CLI or registered preset.
    Harness,
    /// Choose or complete the working directory.
    Workspace,
}

/// One cached workspace completion and why it was suggested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceChoice {
    /// Absolute directory path.
    pub(super) path: String,
    /// Short operator-facing provenance such as `recent` or `folder`.
    pub(super) source: &'static str,
}

/// The "you still hold this harness" confirmation shown on release.
///
/// Modelled on an unsaved-changes prompt, and for the same reason: an operator
/// who took a harness over and walked away has left the orchestrator locked out
/// of it, and the moment they release the keyboard is the only moment they are
/// certainly thinking about it. Silently handing it back would be worse — it
/// would resume dispatch into a harness mid-thought.
pub(super) struct HandbackPrompt {
    /// The session the operator is releasing.
    pub(super) session: String,
    /// Whether attaching is what took control, as opposed to an explicit
    /// `/takecontrol`. An explicit take is a decision, so the prompt says so
    /// rather than implying the operator got here by accident.
    pub(super) took_control: bool,
    /// What the operator wants continued, typed into the prompt.
    ///
    /// This is the moment they actually have the context — they are leaving the
    /// harness *now* — so it is the one place worth asking. `/handoff <note>`
    /// exists for the operator who already knows; this is for the one who is
    /// only reminded by being asked.
    pub(super) note: crate::ui::composer::Draft,
    /// Whether keystrokes are going into the note rather than answering.
    ///
    /// Modal because `y`/`n` have to keep meaning yes and no: an operator who
    /// starts typing a note that begins with "no, ..." must not have the first
    /// letter answer the question for them.
    pub(super) editing_note: bool,
}

/// What to do when the operator releases a harness they hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HandbackPolicy {
    /// Ask, every time.
    #[default]
    Ask,
    /// Always hand back without asking.
    Always,
    /// Never hand back; releasing the keyboard keeps control.
    Never,
}

impl HandbackPolicy {
    /// Parse the `[harness].handback` config value, falling back to
    /// [`Ask`](Self::Ask) for anything unrecognized — a typo in a config file
    /// should not silently change who controls a harness.
    pub fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "always" => HandbackPolicy::Always,
            "never" => HandbackPolicy::Never,
            _ => HandbackPolicy::Ask,
        }
    }
}

/// The action a small inline prompt (Hosts add/edit, Agents answer) submits.
pub(super) enum PromptKind {
    /// Add a worker from an address/@handle line.
    HostAdd,
    /// Edit the label of the worker with the given id.
    HostEditLabel(String),
    /// Declare another directory this device may work in.
    WorkspaceAdd,
    /// Add a named OpenRouter-backed coding harness.
    CustomHarnessAdd,
    /// Edit the custom harness with the given stable id.
    CustomHarnessEdit(String),
    /// The working directory for a new local host, with the harness already
    /// chosen. Blank accepts the default — where this process is running.
    LocalHostWorkspace(medulla::tinyplace::HarnessProvider),
    /// Reject a workflow proposal with the operator's explanation.
    RejectProposal {
        /// The workflow the proposal belongs to.
        workflow: String,
        /// The proposal awaiting the decision.
        proposal_id: String,
    },
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
    /// One field of a workflow's declared inputs, collected before the run
    /// starts. Submitting either opens the prompt for the next field or, when
    /// this was the last, dispatches the run.
    ///
    /// The whole set is carried on the prompt rather than parked in `App`
    /// state, so cancelling with `Esc` abandons the collected values with it —
    /// a half-filled set cannot leak into the next run.
    WorkflowInput {
        /// The workflow the values are being collected for.
        workflow_id: String,
        /// Whether to dispatch a dry run rather than a real one.
        dry_run: bool,
        /// The fields still to ask about; the head is the one on screen.
        remaining: Vec<medulla::workflows::WorkflowInput>,
        /// What has been collected so far, keyed by input name.
        collected: serde_json::Map<String, serde_json::Value>,
    },
}

/// The Feedback surface's state: the loaded page, the selected row, that row's
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
    /// Installed harnesses offered by the Add Host wizard, detected once.
    ///
    /// Detection reads the environment and stat-checks every provider binary on
    /// `PATH`. The wizard asked on every render frame *and* every keypress, so a
    /// page that is drawn at the frame rate was doing filesystem work to answer
    /// a question whose answer cannot change while the process runs.
    pub(super) add_host_provider_cache:
        std::cell::OnceCell<Vec<medulla::tinyplace::HarnessProvider>>,
    /// Selected row on the Routing Hosts page.
    pub(super) host_index: usize,
    /// Whether ↑↓ on the Hosts page drives the role toggles in the preview
    /// rather than the host list above it. Tab moves between the two.
    pub(super) host_roles_focus: bool,
    /// Selected role in the preview's toggle list, while it has focus.
    pub(super) host_role_index: usize,
    /// Selected row on the Routing Workspaces page.
    pub(super) workspace_index: usize,
    /// Selected row on the Routing Agent Templates page.
    pub(super) template_index: usize,
    /// OpenRouter-backed harness presets loaded from the active config.
    pub(super) custom_harnesses: Vec<medulla::config::CustomHarnessConfig>,
    /// Selected row on the Routing Harnesses page.
    pub(super) custom_harness_index: usize,
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
    /// What the selected workflow has learned, newest first.
    ///
    /// Cached beside the runs and refreshed with them, for the same reason: a
    /// render pass must not touch the disk.
    #[cfg(feature = "workflows")]
    pub(super) workflow_notes: Vec<medulla::workflows::WorkflowNote>,
    /// Changes proposed for the selected workflow, newest first.
    #[cfg(feature = "workflows")]
    pub(super) workflow_proposals: Vec<medulla::workflows::WorkflowProposal>,
    /// The Workflows tab's panes, cursors, and copilot threads.
    #[cfg(feature = "workflows")]
    pub(super) wf: WorkflowsState,
    /// A workflow store attached directly, overriding the layered one this
    /// client would otherwise resolve.
    ///
    /// The layered store always reads the current directory's
    /// `.medulla/workflows` as repository defaults, then overlays the
    /// user-global workflow directory. That is useful in a real session and
    /// wrong under test, where it makes the catalogue depend on the developer's
    /// checkout. `None` resolves the layered store, as a real session does.
    #[cfg(feature = "workflows")]
    pub(super) workflow_store_override: Option<Arc<dyn medulla::workflows::WorkflowStore>>,
    /// Which kind of host the Add Host page is offering — a cursor into
    /// [`AddHostKind::ALL`].
    pub(super) add_host_kind: usize,
    /// Which harness a new local host will run — a cursor into the detected
    /// provider list.
    pub(super) add_host_harness: usize,
    /// Whether the kind picker has been answered, so the arrows move on to the
    /// harness list rather than re-picking local versus remote.
    pub(super) add_host_kind_chosen: bool,
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
    /// The active TokenMaxxxing sidebar page.
    pub(super) tokenmaxxing_index: usize,
    /// Whether keyboard focus is inside the TokenMaxxxing content pane.
    pub(super) tokenmaxxing_focused: bool,
    /// Feedback-board state (lazily loaded on entry / refresh).
    pub(super) feedback: FeedbackState,
    /// Feedback-board tab state (lazily loaded on tab entry / refresh).
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
    /// Throttled sampler backing the optional local-process status indicators.
    pub(super) resource_monitor: crate::ui::resources::ResourceMonitor,
    /// The selected field row on the Status line subpage.
    pub(super) status_line_index: usize,
    /// Whether the next persisted status-line edit must write the complete
    /// legacy-derived section rather than one field.
    pub(super) status_line_promotion_pending: bool,
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
    /// Who the embedded core is signed in as, for the Account subpage.
    pub(super) account: Option<medulla::core_host::auth::AuthState>,
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
    /// Where the Agents rail drew, and which rail row each of its visible lines
    /// belongs to. A row may wrap onto several lines, so a click resolves
    /// through this map rather than by adding an offset to a first-row index.
    pub(super) hit_agents: Option<(Rect, Vec<usize>)>,
    // Where the embedded harness screen landed, and whose it is. Recorded so a
    // wheel event can be routed to the terminal under the pointer and given
    // coordinates relative to *its* origin rather than the screen's.
    pub(super) hit_harness: Option<(Rect, String)>,
    /// The threads strip's hit box and its first visible row, for click-to-switch.
    pub(super) hit_threads: Option<(Rect, usize)>,
    pub(super) hit_context: Option<Rect>,
    /// The selected workflow step's preview, for pointer-wheel scrolling.
    pub(super) hit_workflow_preview: Option<Rect>,
    /// Where the active tab's subpage nav drew its page rows. Only one nav is on
    /// screen at a time, so one field serves Routing and Settings.
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
    // The live harness sessions this device is running. `None` when this machine
    // does not host, in which case the Agents tab has no local screen to show
    // and falls back to a remote worker's streamed one, or to the transcript.
    pub(super) harnesses: Option<crate::ui::harness_pane::LocalHarnesses>,
    // Which of the TUI and the selected harness owns the keyboard. Reset to
    // `Chrome` whenever the attached session stops being the selected one, so
    // the operator's keys can never land in a harness they are not looking at.
    pub(super) harness_focus: crate::ui::harness_pane::HarnessFocus,
    // The harness session the Agents pane resolved on the last draw, and the
    // only one the attach chord can act on. Recorded during render because that
    // is where the rail cursor is turned into a selection; cleared at the top of
    // every draw so it can never name a pane that is no longer on screen.
    pub(super) harness_pane_session: Option<String>,
    /// The "start a harness" picker, while it is open.
    pub(super) harness_picker: Option<HarnessPicker>,
    /// The "you still hold this harness" confirmation, while it is open.
    pub(super) handback_prompt: Option<HandbackPrompt>,
    /// How far the Help page is scrolled, in lines.
    pub(super) help_scroll: u16,
    /// What releasing a held harness does, from `[harness].handback`.
    pub(super) handback_policy: HandbackPolicy,
    /// Whether attaching is what took control of the current harness.
    ///
    /// Distinguishes "you picked this up by focusing in" from "you asked for it
    /// with /takecontrol", which the release prompt words differently: the
    /// second was a decision, and re-asking about it as though it were an
    /// accident is how a confirmation becomes noise.
    pub(super) harness_took_control: bool,
    /// Commands raised by synchronous input handlers, drained by the event loop.
    ///
    /// The key and mouse handlers that move harness control cannot return a
    /// [`Cmd`] — `handle_handback_key` returns `()`, `handle_harness_key`
    /// returns `bool`, and the mouse path returns nothing — and threading an
    /// `Option<Cmd>` back through all three would be a wide, test-breaking
    /// change to say one thing. So they push here instead, and the loop drains
    /// it right after the event that produced it. Commands run in submission
    /// order.
    pub(super) pending_cmds: std::collections::VecDeque<Cmd>,
    /// Whether operator-started harnesses launch with the permission-bypass
    /// flag, from `[harness].skipPermissions`.
    pub(super) harness_skip_permissions: bool,
}
