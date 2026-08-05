//! Serde data types for the Medulla hook vocabulary: the canonical lifecycle
//! events, one operator-declared hook, and the per-spawn injection a translator
//! produces. Only shapes and their trivial impls live here — the per-harness
//! translation lives in the sibling adapter modules.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::protocol::HarnessProvider;

/// A lifecycle moment a Medulla hook can attach to.
///
/// The vocabulary is deliberately the harness-native one rather than a Medulla
/// invention. Claude Code 2.1.221 and Codex 0.146 converged on the same event
/// names, the same `matcher` + `hooks[]` grouping, and the same
/// `{"type":"command","command":…}` handler shape, so a canonical event is a
/// rename of nothing — it is the name both harnesses already answer to. The
/// serialized form is that shared wire spelling.
///
/// Where the two disagree, [`HookEvent::supported_by`] is the record of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HookEvent {
    /// Before a tool runs. Can deny the call or rewrite its input.
    PreToolUse,
    /// After a tool returns. Observation and feedback only.
    PostToolUse,
    /// When the harness would ask the operator to approve something.
    PermissionRequest,
    /// A prompt was submitted, before the model sees it.
    UserPromptSubmit,
    /// The main agent finished its turn.
    Stop,
    /// A delegated sub-agent started.
    SubagentStart,
    /// A delegated sub-agent finished.
    SubagentStop,
    /// Before the transcript is compacted.
    PreCompact,
    /// After the transcript is compacted.
    PostCompact,
    /// A session began.
    SessionStart,
    /// A session ended.
    SessionEnd,
    /// The harness surfaced a notification to the operator.
    Notification,
}

impl HookEvent {
    /// Every event in the canonical vocabulary, in declaration order.
    pub const ALL: [HookEvent; 12] = [
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::PermissionRequest,
        HookEvent::UserPromptSubmit,
        HookEvent::Stop,
        HookEvent::SubagentStart,
        HookEvent::SubagentStop,
        HookEvent::PreCompact,
        HookEvent::PostCompact,
        HookEvent::SessionStart,
        HookEvent::SessionEnd,
        HookEvent::Notification,
    ];

    /// The wire spelling both Claude Code and Codex use for this event.
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PermissionRequest => "PermissionRequest",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::Stop => "Stop",
            HookEvent::SubagentStart => "SubagentStart",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::PreCompact => "PreCompact",
            HookEvent::PostCompact => "PostCompact",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::Notification => "Notification",
        }
    }

    /// Parse a canonical event name, accepting the wire spelling only.
    pub fn from_wire(value: &str) -> Option<HookEvent> {
        HookEvent::ALL
            .into_iter()
            .find(|event| event.as_str() == value)
    }

    /// Whether `provider` implements this event.
    ///
    /// Verified against Claude Code 2.1.221 and Codex 0.146. The two sets are
    /// identical but for `Notification`, which only Claude Code raises. An
    /// unsupported pair is dropped at translation and reported through
    /// [`HookInjection::dropped`] rather than silently discarded — a hook the
    /// operator believes is running on every harness, but is not, is the failure
    /// mode this whole module exists to prevent.
    ///
    /// OpenCode and OpenHuman return `false` throughout: OpenCode's hook surface
    /// is a scripted plugin API rather than a declarative command hook, and is
    /// not yet adapted; OpenHuman runs in-process and has no external harness to
    /// configure.
    pub fn supported_by(self, provider: HarnessProvider) -> bool {
        match provider {
            HarnessProvider::Claude => true,
            HarnessProvider::Codex => self != HookEvent::Notification,
            HarnessProvider::Opencode | HarnessProvider::Openhuman => false,
        }
    }
}

/// What a hook runs. Only external commands are modelled: a command is the one
/// handler shape both harnesses accept, and it keeps hook bodies out of Medulla's
/// own process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum HookHandler {
    /// Run a shell command. It receives the harness's native hook payload on
    /// stdin and speaks the harness's native decision protocol on stdout.
    #[serde(rename = "command")]
    Command {
        /// The command line, run through the operator's shell.
        command: String,
        /// Seconds before the harness abandons the hook. Harness default when
        /// absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
}

/// One operator-declared hook: an event, what it matches, and what it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSpec {
    /// The lifecycle moment this hook attaches to.
    pub event: HookEvent,
    /// Tool-name pattern for tool-scoped events. `*` (the default) matches every
    /// tool; ignored by events that carry no tool.
    #[serde(default = "match_all")]
    pub matcher: String,
    /// What the hook runs.
    #[serde(flatten)]
    pub handler: HookHandler,
    /// Restrict this hook to specific harnesses. Empty (the default) means every
    /// harness that supports the event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<HarnessProvider>,
    /// Operator-facing name, shown on the Hooks page in place of the command
    /// line. Absent means the page falls back to the command itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether Medulla supplied this hook itself (see [`super::builtin`]).
    ///
    /// Never read from or written to a config file: it is decided at load, and
    /// it is what keeps a built-in out of the file when the Hooks page saves
    /// the operator's own hooks back.
    #[serde(skip)]
    pub builtin: bool,
}

fn match_all() -> String {
    "*".to_string()
}

impl HookSpec {
    /// Whether this hook should be installed into `provider`.
    ///
    /// Both gates must pass: the operator's own `harnesses` restriction, and the
    /// harness actually implementing the event.
    pub fn applies_to(&self, provider: HarnessProvider) -> bool {
        let allowed = self.harnesses.is_empty() || self.harnesses.contains(&provider);
        allowed && self.event.supported_by(provider)
    }

    /// The command line this hook runs.
    pub fn command(&self) -> &str {
        match &self.handler {
            HookHandler::Command { command, .. } => command,
        }
    }

    /// The hook's timeout in seconds, when it declared one.
    pub fn timeout(&self) -> Option<u64> {
        match &self.handler {
            HookHandler::Command { timeout, .. } => *timeout,
        }
    }

    /// What the Hooks page shows for this hook: its label, or its command.
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or_else(|| self.command())
    }

    /// Render this hook as the Hooks page's one-line editor form.
    ///
    /// The inverse of [`HookSpec::from_editor_line`]; see it for the format and
    /// why the matcher is written with commas here.
    pub fn editor_line(&self) -> String {
        let harnesses = self
            .harnesses
            .iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{} | {} | {} | {} | {}",
            self.event.as_str(),
            self.matcher.replace('|', ","),
            harnesses,
            self.timeout().map(|t| t.to_string()).unwrap_or_default(),
            self.command(),
        )
    }

    /// Parse the Hooks page's one-line editor form.
    ///
    /// `Event | matcher | harnesses | timeout | command`, where every field but
    /// the event and the command may be empty:
    ///
    /// ```text
    /// PostToolUse | Edit,Write | claude | 30 | ./bin/auto-commit
    /// Stop |  |  |  | notify-send "turn done"
    /// ```
    ///
    /// The command is everything after the fourth separator, so a shell pipeline
    /// needs no escaping — which is also why the *matcher* is written with
    /// commas and translated to the harnesses' `|` alternation here. A matcher
    /// that had to carry a literal pipe would make the one field operators write
    /// most often the one they cannot type.
    pub fn from_editor_line(line: &str) -> Result<Self, String> {
        let fields: Vec<&str> = line.splitn(5, '|').collect();
        if fields.len() != 5 {
            return Err(EDITOR_LINE_FORMAT.into());
        }
        let event = HookEvent::from_wire(fields[0].trim()).ok_or_else(|| {
            format!(
                "'{}' is not a lifecycle event; one of: {}",
                fields[0].trim(),
                HookEvent::ALL
                    .iter()
                    .map(|event| event.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        let matcher = match fields[1].trim() {
            "" => match_all(),
            matcher => matcher.replace(',', "|"),
        };
        let harnesses = fields[2]
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| {
                HarnessProvider::from_wire(name)
                    .ok_or_else(|| format!("'{name}' is not a harness Medulla launches"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let timeout = match fields[3].trim() {
            "" => None,
            value => Some(
                value
                    .parse::<u64>()
                    .map_err(|_| format!("'{value}' is not a number of seconds"))?,
            ),
        };
        let command = fields[4].trim().to_string();
        if command.is_empty() {
            return Err("a hook needs a command to run".into());
        }
        Ok(HookSpec {
            event,
            matcher,
            handler: HookHandler::Command { command, timeout },
            harnesses,
            label: None,
            builtin: false,
        })
    }
}

/// The editor form, quoted back at an operator who mistyped it.
const EDITOR_LINE_FORMAT: &str = "Event | matcher | harnesses | timeout | command";

/// The `[[hooks]]` config section: every hook Medulla installs into the
/// harnesses it launches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HooksConfig {
    /// The declared hooks, in config order. Order is preserved into the harness
    /// so a hook that depends on an earlier one keeps its position.
    pub hooks: Vec<HookSpec>,
}

impl HooksConfig {
    /// Whether any hook is declared.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// The hooks that apply to `provider`, in config order.
    pub fn for_provider(&self, provider: HarnessProvider) -> Vec<&HookSpec> {
        self.hooks
            .iter()
            .filter(|hook| hook.applies_to(provider))
            .collect()
    }

    /// Only the hooks an operator declared, in config order.
    ///
    /// The set a config file should contain: built-ins are resolved at load and
    /// writing them back would freeze today's defaults into the operator's file,
    /// where a later Medulla could neither update nor withdraw them.
    pub fn operator_hooks(&self) -> Vec<HookSpec> {
        self.hooks
            .iter()
            .filter(|hook| !hook.builtin)
            .cloned()
            .collect()
    }

    /// Put `builtin` ahead of the operator's own hooks.
    ///
    /// Ahead rather than behind because a built-in only observes: it reports
    /// what happened and never decides, so an operator hook that *does* decide
    /// should see the event with the report already on its way. Existing
    /// built-ins are replaced, so resolving twice is not additive.
    pub fn with_builtin(&self, builtin: Vec<HookSpec>) -> HooksConfig {
        let mut hooks = builtin;
        hooks.extend(self.operator_hooks());
        HooksConfig { hooks }
    }
}

/// One hook that could not be installed, and why.
///
/// Surfaced rather than swallowed so an operator can see that a hook they
/// declared does not cover every harness they run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedHook {
    /// The event that could not be installed.
    pub event: HookEvent,
    /// The harness that does not implement it.
    pub provider: HarnessProvider,
    /// Human-readable reason, for logs and the settings UI.
    pub reason: String,
}

/// What a spawning harness needs in order to run the operator's Medulla hooks:
/// extra argv entries, extra environment, and the hooks that did not survive
/// translation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookInjection {
    /// Arguments prepended to the child argv.
    pub args: Vec<String>,
    /// Environment entries merged into the child environment.
    pub env: HashMap<String, String>,
    /// Hooks that this harness cannot run.
    pub dropped: Vec<DroppedHook>,
    /// Conditions that stop a *supported* hook from taking effect, phrased for
    /// an operator. Distinct from [`Self::dropped`], which records a hook the
    /// harness has no event for at all.
    pub warnings: Vec<String>,
}

impl HookInjection {
    /// Whether this injection changes anything about the spawn.
    pub fn is_empty(&self) -> bool {
        self.args.is_empty() && self.env.is_empty()
    }

    /// Every operator-facing note this injection produced — dropped hooks and
    /// warnings alike — as lines ready for a log or status surface.
    pub fn notes(&self) -> Vec<String> {
        self.dropped
            .iter()
            .map(|dropped| {
                format!(
                    "hook {} not installed for {}: {}",
                    dropped.event.as_str(),
                    dropped.provider.as_str(),
                    dropped.reason,
                )
            })
            .chain(self.warnings.iter().cloned())
            .collect()
    }
}
