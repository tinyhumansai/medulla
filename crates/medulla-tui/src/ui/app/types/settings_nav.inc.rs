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
/// Workflows is a tab because it is *work* — a plan an operator reads, edits,
/// and runs, with a graph to navigate and a copilot to edit it by. Three panes'
/// worth of surface does not fit in a subpage of something else.
///
/// `Hosts` is gone. It was the fleet's management surface — the hosts registered
/// on it, the harness types and their credentials, the hook declarations, the
/// agent-template catalog, host onboarding, and the routing strategies — and all
/// of it existed to feed an orchestration engine that no longer exists. Most of
/// it survives as config: `[[hosts]]`, `[[harnesses]]` and `[[hooks]]` are still
/// read at load, so the runtime behaves exactly as before for those; there is
/// simply no page that edits them by hand any more. The agent-template catalog
/// did not survive — the `.medulla/agents` store and its loader are gone with
/// the tab, so any templates it held are no longer read.
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
pub const TABS: [&str; 6] = [
    "Sessions",
    "Workflows",
    "Subconscious",
    "Resources",
    "Feedback",
    "Settings",
];

/// Without the workflow engine. A slim build must not offer a tab that cannot
/// draw anything.
///
/// This is the workflow-enabled list minus `Workflows`: every remaining tab —
/// the Sessions rail with its `d` diff pane, the live Subconscious signal
/// field, Feedback, and Settings — renders without the workflow engine.
#[cfg(not(feature = "workflows"))]
pub const TABS: [&str; 5] = ["Sessions", "Subconscious", "Resources", "Feedback", "Settings"];

/// The TokenMaxxxing tab's sidebar pages.
pub(in crate::ui::app) const TOKENMAXXING_SUBPAGES: [&str; 3] =
    ["Overview", "Bounties", "Leaderboard"];

pub(in crate::ui::app) const TM_OVERVIEW: usize = 0;
pub(in crate::ui::app) const TM_BOUNTIES: usize = 1;
pub(in crate::ui::app) const TM_LEADERBOARD: usize = 2;

/// The Settings tab's left-nav subpages, in order (number keys 1-9 jump to them).
///
/// This is the flat, selectable list [`App::settings_index`] indexes into.
/// [`SETTINGS_GROUPS`] overlays the display-only headings.
pub const SETTINGS_SUBPAGES: [&str; 13] = [
    "Usage",
    "Tokens",
    "Harnesses",
    "Appearance",
    "Subscriptions",
    "Status line",
    "Config",
    "Resources",
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
pub(in crate::ui::app) const SP_TOKENS: usize = 1;
pub(in crate::ui::app) const SP_HARNESSES: usize = 2;
pub(in crate::ui::app) const SP_APPEARANCE: usize = 3;
pub(in crate::ui::app) const SP_SUBSCRIPTIONS: usize = 4;
pub(in crate::ui::app) const SP_STATUS_LINE: usize = 5;
pub(in crate::ui::app) const SP_CONFIG: usize = 6;
pub(in crate::ui::app) const SP_RESOURCES: usize = 7;
pub(in crate::ui::app) const SP_FEEDBACK: usize = 8;
pub(in crate::ui::app) const SP_TRACE: usize = 9;
pub(in crate::ui::app) const SP_CONTEXT: usize = 10;
pub(in crate::ui::app) const SP_ACCOUNT: usize = 11;
pub(in crate::ui::app) const SP_HELP: usize = 12;

/// The index of a tab by name, or 0 if unknown. Keeps tab jumps robust as the tab
/// list grows.
pub(in crate::ui::app) fn tab_pos(name: &str) -> usize {
    TABS.iter().position(|t| *t == name).unwrap_or(0)
}
