//! Building the agent harness one turn runs on.
//!
//! # Why the model is always OpenAI-compatible
//!
//! Every route Medulla resolves — the account's own inference proxy, an
//! OpenRouter preset, a self-hosted endpoint — speaks the OpenAI chat-completions
//! shape, and [`crate::daemon::providers`] already resolves a preset down to a
//! `(base_url, token)` pair for exactly that reason. One provider type therefore
//! covers every case, and a second would be a second thing to keep in step with
//! no route asking for it.

use std::sync::Arc;
use std::time::Duration;

use tinyagents::runtime::{AgentHarness, PayloadCapture, RunPolicy};
use tinyinference::providers::openai::OpenAiModel;

use super::tools::{self, Workspace};

/// Registry name the turn's model is bound under.
///
/// A fixed name rather than the model id: the id is a *route* detail that
/// changes per preset, and threading it through as the registry key would make
/// the default-model lookup depend on which endpoint answered.
pub const MODEL_KEY: &str = "turn";

/// Where a turn's model calls go.
#[derive(Debug, Clone)]
pub struct Route {
    /// OpenAI-compatible base URL, without a trailing slash.
    pub base_url: String,
    /// Bearer presented to that endpoint.
    pub token: String,
    /// Provider-side model id.
    pub model: String,
}

/// How much a turn may do before it is cut off.
///
/// Separate from the wall-clock deadline the caller supervises: these bound the
/// *shape* of the run (how many times it may loop) where the deadline bounds its
/// duration. A turn that is looping uselessly hits these long before a timeout
/// that is generous enough for one slow build.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Most model calls in one turn.
    pub max_model_calls: usize,
    /// Most tool invocations in one turn.
    pub max_tool_calls: usize,
    /// Per-command deadline for the shell tool.
    pub shell_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Enough for a read-edit-verify cycle with several rounds of
            // correction; far short of a loop that has stopped making progress.
            max_model_calls: 40,
            max_tool_calls: 120,
            shell_timeout: Duration::from_secs(120),
        }
    }
}

/// Build a harness that runs one turn against `route` inside `workspace`.
///
/// The returned harness owns its tools, so it is scoped to that checkout and
/// must not be reused for a turn against a different one.
pub fn build(
    route: &Route,
    workspace: Workspace,
    limits: &Limits,
    env: std::collections::HashMap<String, String>,
) -> AgentHarness<(), ()> {
    let model = OpenAiModel::new(route.token.clone())
        .with_base_url(route.base_url.trim_end_matches('/').to_string())
        .with_model(route.model.clone());

    let mut harness = AgentHarness::<(), ()>::new();
    harness.register_model(MODEL_KEY, Arc::new(model));
    harness.set_default_model(MODEL_KEY);
    // Tool I/O capture is OFF by default in tinyagents, and deliberately so —
    // a shared observability pipeline should not be handed tool arguments it
    // never asked for. Here the only consumer is this host's own run view,
    // which exists to show exactly that: without capture, a transcript renders
    // the *names* of the tools a turn called and none of what they did, which
    // is the difference between a log and a thing an operator can debug.
    //
    // Model I/O stays off: the prompt and completion already reach the
    // transcript as their own rows, so capturing them again would double every
    // turn's payload for nothing.
    harness.with_policy(RunPolicy {
        capture: PayloadCapture {
            model_io: false,
            tool_io: true,
        },
        ..RunPolicy::default()
    });
    for tool in tools::all(workspace, limits.shell_timeout, env) {
        harness.register_tool(tool);
    }
    harness
}

/// The system prompt a local turn runs under.
///
/// Deliberately short. A long persona competes with the workflow node's own
/// instruction for the model's attention, and the node is the thing the operator
/// actually wrote.
pub fn system_prompt(workspace: &Workspace) -> String {
    format!(
        "You are a coding agent working inside the checkout at {}.\n\
         Use `list_dir` and `read_file` to understand the code before changing it, \
         `write_file` to edit, and `shell` to build and test.\n\
         Paths are relative to the checkout and must stay inside it.\n\
         When the task is done, reply with a short summary of what you changed.",
        workspace.root.display()
    )
}
