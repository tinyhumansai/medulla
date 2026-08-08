//! Modal state for the manual-launcher picker and the small prompt/overlay
//! surfaces the launcher and session control raise.
//!
//! The harness/workspace launcher ([`SessionPicker`], [`ResumePicker`],
//! [`SessionPickerStep`], [`WorkspaceChoice`]) plus the single-line prompt
//! overlays ([`Prompt`]/[`PromptKind`]) and the session-release question
//! ([`HandbackPrompt`], [`TakeOrigin`], [`HandbackPolicy`]) sit here rather
//! than in [`super::model`], which holds the screen model itself and is at its
//! size ceiling. All are re-exported through [`super`] as `types::*`.

use ratatui::layout::Rect;

use crate::ui::composer::TextPrompt;
use medulla::client::FeedbackType;


/// The modal state for the "resume a chat" picker overlay.
pub(in crate::ui::app) struct ResumePicker {
    /// The resumable chats to choose from.
    pub(in crate::ui::app) chats: Vec<crate::ui::chat_store::MainChatSummary>,
    /// The highlighted row.
    pub(in crate::ui::app) index: usize,
}

/// An overlay the app can draw over the content pane.
///
/// Ordered as they stack, back to front: the two that float over the content,
/// then the session picker, then the question asked about a session being
/// released, and finally the two that claim a row of their own below it.
///
/// Produced by [`App::visible_overlays`], which is the single source of truth
/// for what is in front of the content — see [`super::super::overlays`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui::app) enum Overlay {
    /// The prepared-decision board.
    Decisions,
    /// The agent-template detail popup.
    TemplatePopup,
    /// The "start a session" picker.
    SessionPicker,
    /// The question asked when the operator lets go of a session.
    HandbackPrompt,
    /// The shared single-line prompt (Workers add/edit, Agents answer).
    InlinePrompt,
    /// The saved-chat resume picker.
    ResumePicker,
}

/// The modal state for the harness-type/workspace picker overlay.
///
/// It answers exactly one question — which CLI, in which directory — and
/// confirming the second step starts the session. It used to carry a
/// `PickerPurpose` because the same two steps also declared an agent, so the end
/// of the flow depended on which door had opened it; the declaration flow is
/// gone, and with one ending there is nothing left to carry.
pub(in crate::ui::app) struct SessionPicker {
    /// Installed providers and registered presets, in offer order.
    pub(in crate::ui::app) choices: Vec<crate::ui::harness_pane::HarnessChoice>,
    /// The highlighted row.
    pub(in crate::ui::app) index: usize,
    /// Which half of the two-step picker owns the keyboard.
    pub(in crate::ui::app) step: SessionPickerStep,
    /// Default directory used to seed the editable workspace query.
    pub(in crate::ui::app) cwd: String,
    /// Inline fuzzy-completion text on the workspace step.
    pub(in crate::ui::app) workspace_query: String,
    /// Cached workspace rows, refreshed only when the query changes.
    pub(in crate::ui::app) workspace_choices: Vec<WorkspaceChoice>,
    /// Highlighted workspace completion.
    pub(in crate::ui::app) workspace_index: usize,
    /// Whether the operator has deliberately picked one of the completions.
    ///
    /// Distinct from `workspace_index != 0`, which cannot express it: a query
    /// that offers a single completion leaves the cursor on row zero however
    /// deliberately it was moved there. Set by the arrows, cleared whenever the
    /// query changes, and read by
    /// [`selected_picker_workspace`](App::selected_picker_workspace) to decide
    /// whether an entered directory outranks the completions listed under it.
    pub(in crate::ui::app) workspace_picked: bool,
}

/// Active stage of the manual session launcher.
///
/// There is deliberately no "managed or unmanaged?" stage. A session the
/// operator starts by hand is theirs — that is what starting it by hand *means*
/// — and the orchestrator spawns its own sessions managed without asking
/// anybody. So the question only ever had one sensible answer, and asking it
/// bought a keystroke, an extra screen, and a freshly started session the
/// operator then had to take back from the orchestrator before typing into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) enum SessionPickerStep {
    /// Choose an installed CLI or registered preset.
    Harness,
    /// Choose or complete the working directory.
    Workspace,
}

/// One cached workspace completion and why it was suggested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ui::app) struct WorkspaceChoice {
    /// Absolute directory path.
    pub(in crate::ui::app) path: String,
    /// Short operator-facing provenance such as `favorite`, `recent`, or `folder`.
    pub(in crate::ui::app) source: String,
    /// An operator-defined favorite name, when this is a saved shortcut.
    pub(in crate::ui::app) label: Option<String>,
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
pub(in crate::ui::app) struct PointerGrab {
    /// The session that received the press.
    pub(in crate::ui::app) session: String,
    /// The button that went down, so a second button's events are not stolen.
    pub(in crate::ui::app) button: crate::ui::harness_pane::mouse::Button,
    /// Where that session's pane was when the press landed.
    ///
    /// Carried rather than re-read from `hit_session` because the grab has to
    /// outlive the pane: the click that opened a modal, detached the harness,
    /// or scrolled the rail can move or remove the rect before the release
    /// arrives, and the release still has to be encoded against the geometry
    /// the child believes it has.
    pub(in crate::ui::app) rect: Rect,
}

/// The "you still hold this session" confirmation shown on release.
///
/// Modelled on an unsaved-changes prompt, and for the same reason: an operator
/// who took a session over and walked away has left the orchestrator locked out
/// of it, and the moment they release the keyboard is the only moment they are
/// certainly thinking about it. Silently handing it back would be worse — it
/// would resume dispatch into a session mid-thought.
pub(in crate::ui::app) struct HandbackPrompt {
    /// The session the question is about.
    ///
    /// Every answer acts on this, never on whatever the rail last resolved: the
    /// question can outlive the frame that raised it, and a `y` that moved
    /// control of a *different* session is the worst outcome this whole flow
    /// has.
    pub(in crate::ui::app) session: String,
    /// Whether attaching is what took control, as opposed to an explicit
    /// `/takecontrol`. An explicit take is a decision, so the prompt says so
    /// rather than implying the operator got here by accident.
    pub(in crate::ui::app) took_control: bool,
    /// What the operator wants continued, typed into the prompt.
    ///
    /// This is the moment they actually have the context — they are leaving the
    /// session *now* — so it is the one place worth asking. `/handoff <note>`
    /// exists for the operator who already knows; this is for the one who is
    /// only reminded by being asked.
    pub(in crate::ui::app) note: crate::ui::composer::Draft,
    /// Whether keystrokes are going into the note rather than answering.
    ///
    /// Modal because `y`/`n` have to keep meaning yes and no: an operator who
    /// starts typing a note that begins with "no, ..." must not have the first
    /// letter answer the question for them.
    pub(in crate::ui::app) editing_note: bool,
    /// Which direction the question is about: `true` asks whether to take the
    /// session from the orchestrator, `false` whether to hand it back.
    ///
    /// One prompt for both because they are the same decision seen from either
    /// side, and the answer is the same keystroke — but the sentence has to say
    /// which way control is about to move, or the operator confirms the
    /// opposite of what they meant.
    pub(in crate::ui::app) is_takeover: bool,
}

/// How the operator came to hold a session the orchestrator had.
///
/// Only the wording of the release question turns on this — both origins ask,
/// because both locked dispatch out of a workspace. What does *not* appear here
/// is "started it myself": that session was never taken from anyone, so it is
/// absent from [`App::sessions_taken`] rather than being a third variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::app) enum TakeOrigin {
    /// Focusing in took it, which the operator may not have realised.
    Focus,
    /// `/takecontrol`, `Ctrl-G`, or answering the takeover question — a decision.
    Explicit,
}

/// What to do when the operator releases a session they took.
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
pub(in crate::ui::app) enum PromptKind {
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
    /// Save the selected manual-launcher directory under an operator-chosen name.
    FavoriteWorkspaceAdd(String),
    /// Add a named OpenRouter-backed coding harness.
    CustomHarnessAdd,
    /// Edit the custom harness with the given stable id.
    CustomHarnessEdit(String),
    /// Declare a lifecycle hook for every harness Medulla launches.
    HookAdd,
    /// Edit the hook at the given row of the Hooks page.
    HookEdit(usize),
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
/// A single-line inline input overlay shared with daemon controls.
pub(in crate::ui::app) type Prompt = TextPrompt<PromptKind>;
