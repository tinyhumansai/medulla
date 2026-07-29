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
    /// `/quit`, `/exit`, `/q` — exit the application.
    Quit,
    /// `/new` — start a fresh conversation session.
    NewSession,
    /// `/resume` — open the saved-chat picker.
    Resume,
    /// `/harness [provider] [path]` — start a harness the orchestrator will not
    /// dispatch into.
    ///
    /// Both arguments are optional: with neither, the front end opens its
    /// picker. Parsing does not validate the path — only the front end knows
    /// what the active workspace is, and a path that does not exist is a
    /// spawn-time error with a much better message than a parse-time one.
    NewHarness {
        /// The harness CLI to run, lowercased, when one was named.
        provider: Option<String>,
        /// The working directory to start it in, when one was given.
        path: Option<String>,
    },
    /// `/takecontrol` — take the selected harness from the orchestrator.
    TakeControl,
    /// `/handoff` — give the selected harness back to the orchestrator.
    HandOff,
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
    /// `/memory [query]`, `/mem [query]` — open the Memory tab. The persona
    /// layer is out of the build, so the query is parsed and ignored rather
    /// than the command disappearing from under an operator's fingers.
    Memory(Option<String>),
    /// `/mouse` — toggle mouse capture.
    ToggleMouse,
    /// `/copy [all|last]` — copy the transcript at the given scope.
    Copy(CopyScope),
    /// A recognized command invoked with an invalid argument; carries the usage
    /// hint to surface.
    BadUsage(&'static str),
    /// An unrecognized command; carries the original trimmed input for the error
    /// message.
    Unknown(String),
}
