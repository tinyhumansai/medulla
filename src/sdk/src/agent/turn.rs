//! Driving one local agent turn.
//!
//! # Why this does not fold the event stream
//!
//! It used to, and that was the wrong seam. Medulla already has one vocabulary
//! for what a turn is doing — [`crate::protocol::HarnessEventKind`], which every
//! spawned CLI harness maps into so the run view renders any of them without
//! knowing which provider ran. A fold here would have been a *second* one, and
//! the consumer would then have had to translate between them.
//!
//! So this forwards [`AgentEvent`] verbatim and lets the consumer map it. The
//! mapping for the daemon provider lives beside its sibling line-mappers, in
//! `daemon::providers::local`, which is where a reader comparing the
//! embedded harness with a spawned one would look.

use std::sync::Arc;

use tinyagents::context::{RunConfig, RunContext};
use tinyagents::events::{AgentEvent, EventListener, EventRecord, EventSink};
use tinyinference::message::{AssistantMessage, Message};

use super::harness::{self, Limits, Route};
use super::tools::Workspace;

/// What a completed turn produced.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    /// The final assistant text.
    pub reply: String,
    /// Input and output tokens, when the provider reported them.
    pub usage: Option<(u64, u64)>,
}

/// Forwards every harness event to the caller's sink.
///
/// A listener rather than a post-hoc walk of the finished run: the rows must
/// reach the run view *while* the turn is producing, and a transcript assembled
/// at the end is one nobody watched. It is also what makes the caller's idle
/// watchdog an idle watchdog rather than a stopwatch — a turn emitting deltas
/// is alive even when it has not finished.
struct Forwarder {
    sink: Arc<dyn Fn(AgentEvent) + Send + Sync>,
}

impl EventListener for Forwarder {
    fn on_event(&self, record: &EventRecord) {
        (self.sink)(record.event.clone());
    }
}

/// Everything one turn needs.
///
/// A struct rather than a parameter list because a turn takes eight distinct
/// things and three of them are string-ish: assembled by name, a caller cannot
/// silently transpose the prompt and the thread id.
pub struct TurnRequest<'a> {
    /// The instruction to run.
    pub prompt: &'a str,
    /// Where the model calls go.
    pub route: &'a Route,
    /// The checkout the tools are rooted at.
    pub workspace: Workspace,
    /// How much the turn may do before it is cut off.
    pub limits: &'a Limits,
    /// Continuity key. A thread seen before replays what was said.
    pub thread_id: &'a str,
    /// The account home the thread's transcript lives under.
    pub home: &'a std::path::Path,
    /// The shell tool's entire environment — the caller's curated map, not this
    /// process's. See [`super::tools::shell`].
    pub env: std::collections::HashMap<String, String>,
}

/// Run one turn.
///
/// `on_event` receives every [`AgentEvent`] the harness emits, as it is emitted.
///
/// # Errors
///
/// Returns a sentence when the harness itself fails — an unreachable endpoint, a
/// rejected credential, a limit tripped before any answer. A failing *tool* is
/// not one of these: it is reported as an event and the model gets to react.
pub async fn run(
    request: TurnRequest<'_>,
    on_event: Arc<dyn Fn(AgentEvent) + Send + Sync>,
) -> Result<TurnOutcome, String> {
    let TurnRequest {
        prompt,
        route,
        workspace,
        limits,
        thread_id,
        home,
        env,
    } = request;
    let agent = harness::build(route, workspace.clone(), limits, env);

    let sink = EventSink::with_stream_id(thread_id);
    sink.subscribe(Arc::new(Forwarder { sink: on_event }));

    let config = RunConfig::new(thread_id)
        .with_thread(thread_id)
        .with_max_model_calls(limits.max_model_calls)
        .with_max_tool_calls(limits.max_tool_calls);
    let ctx = RunContext::new(config, ()).with_events(sink);

    // Prior turns first, so a resumed conversation can refer to what was said.
    // The system prompt leads regardless — it describes the checkout this turn
    // runs in, which a replayed transcript from an earlier turn may not match.
    let prior = super::history::load(home, thread_id);
    let resumed = !prior.is_empty();
    let mut input = Vec::with_capacity(prior.len() + 2);
    input.push(Message::system(harness::system_prompt(&workspace)));
    input.extend(prior);
    input.push(Message::user(prompt));
    if resumed {
        tracing::debug!("[agent] resumed thread {thread_id} with prior messages");
    }

    // Streaming rather than unary: the deltas are what let a consumer show a
    // turn working, and what let its watchdog tell working from idle.
    let run = agent
        .invoke_streaming_in_context(&(), ctx, input)
        .await
        .map_err(|e| format!("the agent turn failed: {e}"))?;

    // Persisted before the reply is returned, so the next turn naming this
    // thread sees it. The system message is dropped on the way in: it is rebuilt
    // per turn from the *current* checkout, and storing it would replay a stale
    // one on top of the fresh one above.
    let transcript: Vec<Message> = run
        .messages
        .iter()
        .filter(|m| !matches!(m, Message::System(_)))
        .cloned()
        .collect();
    super::history::save(home, thread_id, &transcript);

    let reply = run
        .final_response
        .as_ref()
        .map(|r| visible_text(&r.message))
        .unwrap_or_default();

    Ok(TurnOutcome {
        reply,
        usage: Some((run.usage.usage.input_tokens, run.usage.usage.output_tokens)),
    })
}

/// The assistant text an operator should see.
///
/// `ContentBlock::as_text` returns `None` for `Thinking`, which is the point:
/// reasoning is preserved on the message so a provider that requires verbatim
/// replay can round-trip it, but rendering it as the reply would put the model's
/// scratch work in front of the operator as though it were the answer.
fn visible_text(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| block.as_text())
        .collect::<Vec<_>>()
        .join("")
}
