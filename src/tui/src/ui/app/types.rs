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
pub const TABS: [&str; 7] = [
    "Overview",
    "Agents",
    "Workflows",
    "Changes",
    "Hosts",
    "Feedback",
    "Settings",
];

/// Without the workflow engine. A slim build must not offer a tab that cannot
/// draw anything.
#[cfg(not(feature = "workflows"))]
pub const TABS: [&str; 6] = [
    "Overview", "Agents", "Changes", "Hosts", "Feedback", "Settings",
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
    "Harness Types",
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
    /// Vertical scroll of the canvas, in rows.
    ///
    /// The only scroll the canvas has: the graph folds onto a new band whenever
    /// a layer would run past the right edge, so it is never wider than the
    /// pane and there is nothing to scroll horizontally. Counted in rows rather
    /// than lanes because a fold puts a band boundary between two lanes, and a
    /// scroll measured in lanes cannot address the gap.
    pub(super) canvas_row: usize,
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

/// The modal state for the "resume a chat" picker overlay.
pub(super) struct ResumePicker {
    /// The resumable chats to choose from.
    pub(super) chats: Vec<crate::ui::chat_store::MainChatSummary>,
    /// The highlighted row.
    pub(super) index: usize,
}

/// An overlay the app can draw over the content pane.
///
/// Ordered as they stack, back to front: the two that float over the content,
/// then the session picker, then the question asked about a session being
/// released, and finally the two that claim a row of their own below it.
///
/// Produced by [`App::visible_overlays`], which is the single source of truth
/// for what is in front of the content — see [`super::overlays`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Overlay {
    /// The prepared-decision board.
    Decisions,
    /// The agent-template detail popup.
    TemplatePopup,
    /// The "start a session" picker.
    AgentPicker,
    /// The question asked when the operator lets go of a session.
    HandbackPrompt,
    /// The shared single-line prompt (Workers add/edit, Agents answer).
    InlinePrompt,
    /// The saved-chat resume picker.
    ResumePicker,
}

/// What the harness-type/workspace picker is being used for.
///
/// The same two steps — pick a CLI, pick a directory — answer both questions the
/// Agents tab asks, and they differ only in what happens at the end. Declaring an
/// agent writes `harness × workspace` to the config and starts nothing; spawning
/// starts a session and declares nothing. Carrying the intent on the picker keeps
/// one overlay rather than two that would drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PickerPurpose {
    /// Start a session here and now, declaring nothing — the `/session` path.
    Spawn,
    /// Declare an agent: `harness × workspace`, named on the step after.
    DeclareAgent,
}

/// The modal state for the harness-type/workspace picker overlay.
pub(super) struct AgentPicker {
    /// What confirming the last step will do.
    pub(super) purpose: PickerPurpose,
    /// Installed providers and registered presets, in offer order.
    pub(super) choices: Vec<crate::ui::harness_pane::HarnessChoice>,
    /// The highlighted row.
    pub(super) index: usize,
    /// Which half of the two-step picker owns the keyboard.
    pub(super) step: AgentPickerStep,
    /// Default directory used to seed the editable workspace query.
    pub(super) cwd: String,
    /// Inline fuzzy-completion text on the workspace step.
    pub(super) workspace_query: String,
    /// Cached workspace rows, refreshed only when the query changes.
    pub(super) workspace_choices: Vec<WorkspaceChoice>,
    /// Highlighted workspace completion.
    pub(super) workspace_index: usize,
    /// Whether the operator has deliberately picked one of the completions.
    ///
    /// Distinct from `workspace_index != 0`, which cannot express it: a query
    /// that offers a single completion leaves the cursor on row zero however
    /// deliberately it was moved there. Set by the arrows, cleared whenever the
    /// query changes, and read by
    /// [`selected_picker_workspace`](App::selected_picker_workspace) to decide
    /// whether an entered directory outranks the completions listed under it.
    pub(super) workspace_picked: bool,
    /// Whether to spawn managed (orchestrator can dispatch) or unmanaged.
    pub(super) managed: bool,
}

/// Active stage of the manual session launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentPickerStep {
    /// Choose an installed CLI or registered preset.
    Harness,
    /// Choose managed or unmanaged control mode.
    Decision,
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

/// A pointer gesture a harness owns until the button comes back up.
///
/// Terminals grab the pointer on press: every drag and the release belong to
/// whoever took the press, regardless of where the pointer has moved to since.
/// The embedded pane has to do the same, because the alternatives are both
/// visible failures — a release that lands outside the pane, or one swallowed
/// by the hand-back question the click itself opened, leaves the child holding
/// a button nobody is pressing. Claude Code and Codex then read every later
/// motion as a drag and anchor their popups to a press the operator has long
/// since let go of.
#[derive(Clone)]
pub(super) struct PointerGrab {
    /// The session that received the press.
    pub(super) session: String,
    /// The button that went down, so a second button's events are not stolen.
    pub(super) button: crate::ui::harness_pane::mouse::Button,
    /// Where that session's pane was when the press landed.
    ///
    /// Carried rather than re-read from `hit_session` because the grab has to
    /// outlive the pane: the click that opened a modal, detached the harness,
    /// or scrolled the rail can move or remove the rect before the release
    /// arrives, and the release still has to be encoded against the geometry
    /// the child believes it has.
    pub(super) rect: Rect,
}

/// The "you still hold this session" confirmation shown on release.
///
/// Modelled on an unsaved-changes prompt, and for the same reason: an operator
/// who took a session over and walked away has left the orchestrator locked out
/// of it, and the moment they release the keyboard is the only moment they are
/// certainly thinking about it. Silently handing it back would be worse — it
/// would resume dispatch into a session mid-thought.
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
    /// session *now* — so it is the one place worth asking. `/handoff <note>`
    /// exists for the operator who already knows; this is for the one who is
    /// only reminded by being asked.
    pub(super) note: crate::ui::composer::Draft,
    /// Whether keystrokes are going into the note rather than answering.
    ///
    /// Modal because `y`/`n` have to keep meaning yes and no: an operator who
    /// starts typing a note that begins with "no, ..." must not have the first
    /// letter answer the question for them.
    pub(super) editing_note: bool,
    /// Which direction the question is about: `true` asks whether to take the
    /// session from the orchestrator, `false` whether to hand it back.
    ///
    /// One prompt for both because they are the same decision seen from either
    /// side, and the answer is the same keystroke — but the sentence has to say
    /// which way control is about to move, or the operator confirms the
    /// opposite of what they meant.
    pub(super) is_takeover: bool,
}

/// What to do when the operator releases a session they hold.
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
    /// should not silently change who controls a session.
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
    /// Select an arbitrary Git revision as the Changes comparison baseline.
    ChangesBaseline,
    /// Attach a session-local review comment to a file, hunk, or patch line.
    ChangesComment {
        /// Repository-relative path being reviewed.
        path: std::path::PathBuf,
        /// Position within that file's patch the note is bound to.
        anchor: medulla::ui::git_review::CommentAnchor,
    },
    /// Add a worker from an address/@handle line.
    HostAdd,
    /// Edit the label of the worker with the given id.
    HostEditLabel(String),
    /// Declare another directory this device may work in.
    WorkspaceAdd,
    /// Name the agent about to be declared for this `harness × workspace`.
    ///
    /// Blank accepts the id [`suggest_agent_id`](medulla::runtime::suggest_agent_id)
    /// minted from the directory, which is how a person refers to the agent
    /// anyway — the prompt exists for the case where it is not.
    AgentName {
        /// The CLI the agent runs.
        harness: String,
        /// The absolute directory its sessions work in.
        workspace: String,
    },
    /// Name the session about to be opened under an already-declared agent.
    ///
    /// A session a person spins up is [`SessionOrigin::User`](crate::worker::pty::SessionOrigin)
    /// and is the only kind that carries a name; a dispatched one is labelled
    /// from its task. Blank leaves it unnamed rather than inventing one.
    SessionName {
        /// The agent whose harness type and workspace the session inherits.
        agent_id: String,
        /// Whether the orchestrator may dispatch into it — ownership at birth.
        managed: bool,
    },
    /// Add a named OpenRouter-backed coding harness.
    CustomHarnessAdd,
    /// Edit the custom harness with the given stable id.
    CustomHarnessEdit(String),
    /// The working directory for a new local host, with the harness already
    /// chosen. Blank accepts the default — where this process is running.
    LocalHostWorkspace(medulla::protocol::HarnessProvider),
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
    /// Git changes from the selected session or operator-chosen commit.
    pub(super) changes: super::changes::GitChangesState,
    pub(super) draft: Draft,
    pub(super) history: Vec<String>,
    pub(super) history_index: i64,
    pub(super) selected: usize,
    /// The Overview tab's animated workflow graph. Held on the app because its
    /// simulation has to survive between frames; it is advanced by the draw
    /// path, which is the only thing that looks at it.
    pub(super) graph: super::render::graph::Graph,
    pub(super) status: String,
    /// A persistent "update vX.Y.Z available" banner, set by the background
    /// update checker; shown in the header until the app exits.
    pub(super) update_notice: Option<String>,
    pub(super) contexts: Vec<ContextItem>,
    pub(super) context_index: usize,
    pub(super) agent_index: usize,
    /// Extra pages of sublanes revealed under an agent lane, keyed by lane key.
    ///
    /// Keyed by [`AgentLane::key`](crate::ui::agents::AgentLane::key) rather than
    /// by the lane's rail position, because lanes are re-folded from events every
    /// tick and a lane that appears or ends shifts every index below it — an
    /// expansion tied to a position would silently jump to a different agent.
    /// Absent means the lane shows its first page, which is the default every
    /// lane starts at.
    pub(super) subtask_pages: std::collections::HashMap<String, usize>,
    /// The `(worker address, task id)` whose screen is currently subscribed.
    ///
    /// Held so a selection change can stop the old stream as well as start the
    /// new one: a subscription nobody is looking at costs the worker a sample,
    /// a ratchet advance and a send on every tick.
    pub(super) watching: Option<(String, String)>,
    /// The watched `(worker, task)` awaiting destructive-action confirmation.
    pub(super) kill_armed: Option<(String, String)>,
    /// Which half of the Agents tab the keyboard is driving.
    pub(super) agents_focus: AgentsFocus,
    pub(super) agent_scroll: usize,
    pub(super) chat_scroll: usize,
    /// Selected row in the command peek, while it is open.
    pub(super) command_index: usize,
    /// Installed harness types offered by the Add Host wizard, detected once.
    ///
    /// Detection reads the environment and stat-checks every provider binary on
    /// `PATH`. The wizard asked on every render frame *and* every keypress, so a
    /// page that is drawn at the frame rate was doing filesystem work to answer
    /// a question whose answer cannot change while the process runs.
    pub(super) add_host_provider_cache:
        std::cell::OnceCell<Vec<medulla::protocol::HarnessProvider>>,
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
    /// Selected row on the Routing Harness Types page.
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
    /// Which harness type a new local host will run — a cursor into the detected
    /// provider list.
    pub(super) add_host_harness: usize,
    /// Whether the kind picker has been answered, so the arrows move on to the
    /// harness-type list rather than re-picking local versus remote.
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
    /// Throttled sampler backing the optional whole-device sidebar indicators.
    pub(super) device_monitor: crate::ui::resources::DeviceMonitor,
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
    // Where the embedded session screen landed, and whose it is. Recorded so a
    // wheel event can be routed to the terminal under the pointer and given
    // coordinates relative to *its* origin rather than the screen's.
    pub(super) hit_session: Option<(Rect, String)>,
    /// The threads strip's hit box and its first visible row, for click-to-switch.
    pub(super) hit_threads: Option<(Rect, usize)>,
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
    pub(super) hit_started_sessions: Option<(Rect, Vec<Option<String>>)>,
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

    // Optional observational overlay from the background host-link service:
    // this endpoint's own identity, its peer roster, and peer presence. Merged
    // into the snapshot on every refresh so the Overview panel and Agents lanes
    // light up without the runtime having to know about the link.
    pub(super) link_obs: Option<Arc<std::sync::Mutex<medulla::protocol::service::LinkObservation>>>,
    // A read-only view of the task host running on this device, when one is.
    // Read live at render rather than merged into the snapshot: its counters
    // move on the host's own schedule, and the snapshot is the *runtime's*
    // picture of the world — the host is a peer to it, not part of it.
    pub(super) host_obs: Option<medulla::daemon::embedded::HostObservation>,
    // The live sessions this device is running. `None` when this machine
    // does not host, in which case the Agents tab has no local screen to show
    // and falls back to a remote worker's streamed one, or to the transcript.
    pub(super) local_sessions: Option<crate::ui::harness_pane::LocalSessions>,
    // Which of the TUI and the selected session owns the keyboard. Reset to
    // `Chrome` whenever the attached session stops being the selected one, so
    // the operator's keys can never land in a session they are not looking at.
    pub(super) harness_focus: crate::ui::harness_pane::HarnessFocus,
    // The session the Agents pane resolved on the last draw, and the
    // only one the attach chord can act on. Recorded during render because that
    // is where the rail cursor is turned into a selection; cleared at the top of
    // every draw so it can never name a pane that is no longer on screen.
    pub(super) pane_session: Option<String>,
    // The session that took the press of the button currently held down, if any.
    // A terminal grabs the pointer for the whole gesture: whoever received the
    // press receives the drags and the release too, wherever the pointer has
    // wandered to since. Without the grab a release outside the pane — or one
    // swallowed by a modal the click itself opened — never reaches the child,
    // which goes on believing the button is still down and misplaces everything
    // it draws in response to the pointer afterwards.
    pub(super) pointer_grab: Option<PointerGrab>,
    // Where the hand-back question drew each of its answers, and the key each
    // one stands for. Recorded during the draw so a click can be answered by
    // replaying the keystroke rather than by a second copy of the routing: the
    // two would drift, and the direction they would drift in is a pointer that
    // hands a harness back when the operator meant to keep it.
    pub(super) hit_handback: Vec<(Rect, crossterm::event::KeyCode)>,
    // The agent behind a selected session row that this device does NOT run,
    // recorded alongside `pane_session` on the same draw.
    //
    // Its only purpose is to tell "the cursor is not on a session" apart from
    // "the cursor is on somebody else's session", which `pane_session` cannot:
    // both leave it `None`. Taking control resolves through the local workspace
    // path, so a remote session can be watched but not taken (§E7), and an
    // operator who presses the take chord on one deserves that answer rather
    // than "no session on this row".
    pub(super) pane_remote_session: Option<String>,
    // The session selected on the Agents rail, retained while another tab is
    // visible. Unlike `pane_session`, this is navigation state rather than a
    // keyboard-routing capability: Changes uses it to keep following
    // the repository the operator selected after an intervening tab draw.
    pub(super) rail_session: Option<String>,
    /// The "start a session" picker, while it is open.
    pub(super) agent_picker: Option<AgentPicker>,
    /// The "you still hold this session" confirmation, while it is open.
    pub(super) handback_prompt: Option<HandbackPrompt>,
    /// How far the Help page is scrolled, in lines.
    pub(super) help_scroll: u16,
    /// What releasing a held session does, from `[harness].handback`.
    pub(super) handback_policy: HandbackPolicy,
    /// Whether attaching is what took control of the current session.
    ///
    /// Distinguishes "you picked this up by focusing in" from "you asked for it
    /// with /takecontrol", which the release prompt words differently: the
    /// second was a decision, and re-asking about it as though it were an
    /// accident is how a confirmation becomes noise.
    pub(super) took_control_by_attach: bool,
    /// Commands raised by synchronous input handlers, drained by the event loop.
    ///
    /// The key and mouse handlers that move session control cannot return a
    /// [`Cmd`] — `handle_handback_key` returns `()`, `handle_harness_key`
    /// returns `bool`, and the mouse path returns nothing — and threading an
    /// `Option<Cmd>` back through all three would be a wide, test-breaking
    /// change to say one thing. So they push here instead, and the loop drains
    /// it right after the event that produced it. Commands run in submission
    /// order.
    pub(super) pending_cmds: std::collections::VecDeque<Cmd>,
    /// Whether operator-started sessions launch with the permission-bypass
    /// flag, from `[harness].skipPermissions`.
    pub(super) harness_skip_permissions: bool,
}
