//! The parsed slash-command vocabulary and the clipboard scope it can carry.

/// Which part of the chat transcript a `/copy` command targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyScope {
    /// The whole conversation transcript.
    All,
    /// Only the most recent assistant reply.
    Last,
}

/// A parsed slash command entered on the composer line.
///
/// [`super::parse`] turns raw input into one of these; the front end matches on
/// the result to perform the side effect. Parsing is pure and carries no UI
/// state, so the same vocabulary is reusable by any front end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// `/quit`, `/q` — exit the application.
    Quit,
    /// `/new` — start a fresh conversation session.
    NewSession,
    /// `/fork [name]` — fork the current thread, optionally named (original case
    /// preserved).
    Fork(Option<String>),
    /// `/resume` — open the saved-chat picker.
    Resume,
    /// `/abort` — request cancellation of the running cycle.
    Abort,
    /// `/clear` — reset the view (runtime history is retained).
    ClearView,
    /// `/help` — show the help subpage.
    Help,
    /// `/config` — show the config subpage.
    Config,
    /// `/settings`, `/theme` — show the appearance subpage.
    Settings,
    /// `/usage` — show the usage subpage (fetches account usage on entry).
    Usage,
    /// `/memory [query]`, `/mem [query]` — load persona memory, or search it when
    /// a query is given (original case preserved).
    Memory(Option<String>),
    /// `/lesson <trigger> -> <rule>` — append a durable workspace lesson.
    Lesson { trigger: String, rule: String },
    /// `/review <lane|task-id>` — launch an independent fresh-context review.
    Review(String),
    /// `/feedback` — open the feedback board tab.
    Feedback,
    /// `/mouse` — toggle mouse capture.
    ToggleMouse,
    /// `/copy [all|last]` — copy the transcript at the given scope.
    Copy(CopyScope),
    /// `/async [on|off]` — set async delegation mode; `None` toggles it.
    Async(Option<bool>),
    /// A recognized command invoked with an invalid argument; carries the usage
    /// hint to surface.
    BadUsage(&'static str),
    /// An unrecognized command; carries the original trimmed input for the error
    /// message.
    Unknown(String),
}
