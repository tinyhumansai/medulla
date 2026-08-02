//! The fleet tools' descriptions, as a model reads them.
//!
//! Prose, not plumbing. These are the entire briefing a model gets about work it
//! will hand to another machine, and the three facts it cannot recover from if
//! they are left out: the worker has no memory of this conversation and no view
//! of these files, a task routinely runs for minutes, and a task dies with the
//! Medulla instance that accepted it.

use serde_json::{json, Value};

/// The fleet tool definitions, given the shared schema helper.
///
/// `may_dispatch` is false when this session's grant does not cover dispatching
/// — no fleet family, or already at the depth ceiling. The verbs that would
/// start or stop work are then left out of the list entirely rather than
/// advertised and refused: a tool that is absent is a fact a model cannot argue
/// with, where one that is present and always fails reads as a broken server.
pub(crate) fn definitions(
    schema: impl Fn(Value, &[&str]) -> Value,
    may_dispatch: bool,
) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "fleet_status",
            "description":
                "Whether this machine has a running Medulla you can hand work to, how many \
                 workers it has, and how deep in a dispatch tree you already are. Cheap, and \
                 worth calling before you plan around delegating: if there is no fleet, or you \
                 are at the depth limit, do the work yourself rather than waiting on a tool \
                 that will refuse.",
            "inputSchema": schema(json!({}), &[]),
        }),
        json!({
            "name": "fleet_workers",
            "description":
                "The machines this fleet can place work on: the id to name in fleet_dispatch, \
                 the coding harness each one runs, the directory it works in, and whether a \
                 person is currently holding it. Read this before dispatching anything you care \
                 about the placement of — a worker whose workspace does not contain the \
                 repository you mean will do the wrong work convincingly, and one a person has \
                 taken back will refuse the task outright.",
            "inputSchema": schema(json!({}), &[]),
        }),
        json!({
            "name": "fleet_tasks",
            "description":
                "Every task you have dispatched this session, newest first, with its status, \
                 worker, elapsed time, and the opening of its instruction. Where to start after \
                 losing track of a handle, and worth reading before fanning out further — a \
                 fleet already saturated with your own work is a reason to wait rather than \
                 dispatch more. Only your own tasks: work dispatched by another session is not \
                 visible here.",
            "inputSchema": schema(json!({}), &[]),
        }),
    ];

    if !may_dispatch {
        return tools;
    }

    tools.push(json!({
        "name": "fleet_dispatch",
        "description":
            "Hand one self-contained instruction to a worker in this fleet and get back a \
             handle. The worker is a real coding harness on a real machine, with no memory of \
             this conversation and no view of your files — everything it needs (which \
             repository, what change, what would count as done) has to be in the instruction \
             itself. This returns as soon as the task is accepted, NOT when it finishes: a task \
             routinely runs for minutes. Read the outcome with fleet_result, and change your \
             mind with fleet_abort. Tasks live only as long as the Medulla instance that \
             accepted them.",
        "inputSchema": schema(
            json!({
                "instruction": {
                    "type": "string",
                    "description":
                        "The whole brief, written for an agent that knows nothing about this \
                         conversation.",
                },
                "worker": {
                    "type": "string",
                    "description":
                        "Worker id from fleet_workers. Omit to use this host's default.",
                },
                "harness": {
                    "type": "string",
                    "enum": ["claude", "codex", "opencode"],
                    "description":
                        "Which coding CLI runs it. Omit to let the worker use its own default; \
                         a worker that does not have the one you name refuses rather than \
                         substituting another.",
                },
                "model": {
                    "type": "string",
                    "description":
                        "Model hint for that harness. Omit unless the operator named one.",
                },
                "workflow": {
                    "type": "string",
                    "description":
                        "Run this saved workflow instead of handing the instruction to a \
                         harness as a prompt; the instruction becomes its trigger payload. Ids \
                         come from workflow_list.",
                },
                "inputs": {
                    "type": "object",
                    "description":
                        "Values for the selected workflow's declared inputs, keyed by name. Read \
                         the declarations from workflow_get or workflow_list; omit this unless \
                         workflow is set.",
                },
            }),
            &["instruction"],
        ),
    }));

    tools.push(json!({
        "name": "fleet_result",
        "description":
            "Wait for a dispatched task and read its reply. Returns the moment the task \
             settles, or after `waitSeconds` with status \"running\" plus whatever progress the \
             worker has reported — in that case call it again rather than concluding anything, \
             because a task that has been running ten minutes is ordinary. Status is one of \
             running, done, failed, aborted, busy, held, timeout. Only \"failed\" is the task's \
             own error: \"busy\" and \"held\" mean the worker never started it, so nothing was \
             attempted and the same dispatch later would very likely work.",
        "inputSchema": schema(
            json!({
                "taskId": {
                    "type": "string",
                    "description": "The handle fleet_dispatch returned.",
                },
                "waitSeconds": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 120,
                    "default": 25,
                    "description":
                        "How long to block before answering \"still running\". Keep it under \
                         your own tool-call timeout; 0 polls and returns immediately.",
                },
            }),
            &["taskId"],
        ),
    }));

    tools.push(json!({
        "name": "fleet_abort",
        "description":
            "Stop a task you dispatched. The worker is told to cancel and the task settles as \
             \"aborted\". Anything it already wrote to disk stays written — this stops the \
             agent, it does not undo its work, and on a half-finished change that is worth \
             saying to the operator. Only handles from this session can be aborted. Reach for \
             this when the brief was wrong or the operator changed their mind, never as a \
             timeout: a slow task is usually still working, and fleet_result will show you its \
             progress.",
        "inputSchema": schema(
            json!({
                "taskId": {
                    "type": "string",
                    "description": "The handle fleet_dispatch returned.",
                },
            }),
            &["taskId"],
        ),
    }));

    tools
}
