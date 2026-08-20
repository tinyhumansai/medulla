//! The slash-command catalog: every command, its aliases, its argument hint,
//! and the one line that describes it.
//!
//! One list, three consumers: the composer's peek (what you see while typing
//! `/`), the Help page, and the parser's own vocabulary. They used to be three
//! hand-maintained lists, which is how a command ends up parseable but
//! undiscoverable, or listed in help and gone from the parser.
//!
//! Order is the order they are offered in, which is roughly how often a session
//! reaches for them rather than alphabetical: the everyday verbs first, the
//! places to go next, the diagnostics last.

/// One command as the operator meets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// The canonical name, without the leading slash.
    pub name: &'static str,
    /// Accepted alternatives, also without the slash.
    pub aliases: &'static [&'static str],
    /// The argument shape, e.g. `[query]`, or empty when it takes none.
    pub args: &'static str,
    /// One line, imperative, describing what running it does.
    pub description: &'static str,
}

impl CommandSpec {
    /// The command as typed, including its argument hint: `/copy [all|last]`.
    pub fn usage(&self) -> String {
        if self.args.is_empty() {
            format!("/{}", self.name)
        } else {
            format!("/{} {}", self.name, self.args)
        }
    }

    /// Whether `token` names this command, by canonical name or alias.
    ///
    /// Case-insensitive, matching the parser: `/QUIT` and `/quit` are one
    /// command, and the peek must agree with what will actually run.
    pub fn matches_name(&self, token: &str) -> bool {
        self.name.eq_ignore_ascii_case(token)
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(token))
    }

    /// Whether `prefix` is a prefix of this command's name or any alias.
    pub fn starts_with(&self, prefix: &str) -> bool {
        let prefix = prefix.to_ascii_lowercase();
        self.name.starts_with(&prefix)
            || self
                .aliases
                .iter()
                .any(|alias| alias.starts_with(prefix.as_str()))
    }
}

/// Every command the composer accepts, in offer order.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "new",
        aliases: &[],
        args: "",
        description: "Open a new thread beside this one",
    },
    CommandSpec {
        name: "resume",
        aliases: &[],
        args: "",
        description: "Pick up an earlier saved session",
    },
    CommandSpec {
        name: "session",
        aliases: &["harness"],
        args: "[harness] [path]",
        description: "Start a local session",
    },
    CommandSpec {
        name: "abort",
        aliases: &[],
        args: "",
        description: "Stop the running cycle",
    },
    CommandSpec {
        name: "clear",
        aliases: &[],
        args: "",
        description: "Reset the view (history is kept)",
    },
    CommandSpec {
        name: "copy",
        aliases: &[],
        args: "[all|last]",
        description: "Copy the transcript to the clipboard",
    },
    CommandSpec {
        name: "usage",
        aliases: &[],
        args: "",
        description: "Show account token usage",
    },
    CommandSpec {
        name: "settings",
        aliases: &["theme"],
        args: "",
        description: "Open the appearance settings",
    },
    CommandSpec {
        name: "config",
        aliases: &[],
        args: "",
        description: "Show the loaded configuration",
    },
    CommandSpec {
        name: "feedback",
        aliases: &["fb"],
        args: "",
        description: "Open the feedback board",
    },
    CommandSpec {
        name: "mouse",
        aliases: &[],
        args: "",
        description: "Toggle mouse capture (off to select text)",
    },
    CommandSpec {
        name: "help",
        aliases: &[],
        args: "",
        description: "List the commands and keys",
    },
    CommandSpec {
        name: "quit",
        aliases: &["exit", "q"],
        args: "",
        description: "Exit medulla",
    },
];

/// The commands offered for a composer line, or `None` when the line is not a
/// command being typed.
///
/// The peek is for *choosing* a command, so it closes as soon as one has been
/// chosen: a line with an argument (`/copy last`) has moved on to the
/// argument, and a bare exact match still offers itself so the description
/// stays readable until Enter. An unmatched prefix yields an empty list rather
/// than `None` — "no such command" is worth showing, and silently hiding the
/// peek reads as the feature breaking.
pub fn suggestions(line: &str) -> Option<Vec<&'static CommandSpec>> {
    let rest = line.strip_prefix('/')?;
    if rest.contains(char::is_whitespace) {
        return None;
    }
    Some(
        COMMANDS
            .iter()
            .filter(|spec| spec.starts_with(rest))
            .collect(),
    )
}

/// The catalog entry for a parsed command token, if it names one.
pub fn lookup(token: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|spec| spec.matches_name(token))
}
