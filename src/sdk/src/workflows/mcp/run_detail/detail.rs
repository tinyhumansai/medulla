//! Assembling the joined answer: durable record, live dispatches, and a note.

use serde_json::{json, Value};

use crate::control_socket::{ControlError, FleetWorker};
use crate::mcp::McpSession;
use crate::workflows::ops::{self, StepDetail};
use crate::workflows::WorkflowError;

use super::types::{LiveDispatch, Standing};

/// The task-id prefix every `agent` dispatch of `run_id` carries.
///
/// Returned to the caller as well as used for filtering: a reader who wants to
/// grep a worker's own logs needs the same string, and deriving it from the
/// format string here is better than asking them to reconstruct it.
pub(super) fn dispatch_prefix(run_id: &str) -> String {
    format!("wf:{run_id}:")
}

/// Split a task id into the route and sequence its suffix encodes.
///
/// `None` for anything that does not fit the shape. A task id is data from
/// another process, and a hub that one day tags a task differently should drop
/// out of this view rather than be reported with an invented route.
pub(super) fn split_suffix(task_id: &str, prefix: &str) -> Option<(String, u64)> {
    let suffix = task_id.strip_prefix(prefix)?;
    let (route, sequence) = suffix.rsplit_once('#')?;
    Some((route.to_string(), sequence.parse().ok()?))
}

/// This run's live dispatches, drawn from the roster the control plane reports.
pub(super) fn dispatches_in(workers: &[FleetWorker], prefix: &str) -> Vec<LiveDispatch> {
    workers
        .iter()
        .flat_map(|worker| {
            worker.running.iter().filter_map(move |task_id| {
                let (route, sequence) = split_suffix(task_id, prefix)?;
                Some(LiveDispatch {
                    task_id: task_id.clone(),
                    route,
                    sequence,
                    worker: worker.id.clone(),
                    harness: worker.harness.clone(),
                    workspace: worker.workspace.clone(),
                })
            })
        })
        .collect()
}

/// This run's live dispatches as *this process* knows them.
///
/// A run started here dispatches onto an embedded host the outer control plane
/// has no row for, so the roster half of this answer is empty for exactly the
/// runs an MCP caller most often asks about — the ones it started itself. This
/// is the other half; see [`crate::workflows::run::dispatches`].
pub(super) fn dispatches_here(run_id: &str, prefix: &str) -> Vec<LiveDispatch> {
    crate::workflows::run::in_flight(run_id)
        .into_iter()
        .filter_map(|dispatch| {
            let (route, sequence) = split_suffix(&dispatch.task_id, prefix)?;
            Some(LiveDispatch {
                task_id: dispatch.task_id,
                route,
                sequence,
                worker: dispatch.worker,
                harness: dispatch.harness,
                workspace: dispatch.workspace,
            })
        })
        .collect()
}

/// Fold the two halves into one list, in dispatch order.
///
/// A dispatch this process made *to a fleet worker* is seen twice — once by the
/// local registry and once by the roster — and the roster's copy is the better
/// of the two, because it names the machine that actually took the work rather
/// than the address it was routed to. So fleet rows are laid down first and a
/// local row is only kept for a task id the roster did not account for.
pub(super) fn merge(fleet: Vec<LiveDispatch>, local: Vec<LiveDispatch>) -> Vec<LiveDispatch> {
    let mut dispatches = fleet;
    for dispatch in local {
        if !dispatches
            .iter()
            .any(|existing| existing.task_id == dispatch.task_id)
        {
            dispatches.push(dispatch);
        }
    }
    // Dispatch order, which is the order an operator reading this thinks in.
    // Neither half's own order is stable: the roster's is the hub's map
    // iteration, and the local registry's is arrival order across nodes.
    dispatches.sort_by_key(|dispatch| dispatch.sequence);
    dispatches
}

/// Ask the control plane for the roster, or say why it could not be asked.
///
/// A fleet that is unreachable is not a failure of this tool: the durable half
/// of the answer is still worth having, and a caller told "no fleet" can act on
/// that. So every outcome that is not a roster comes back as prose to inline
/// beside the record rather than as an error that loses it.
async fn roster(session: &McpSession) -> Result<Vec<FleetWorker>, String> {
    if !session.families.fleet {
        return Err(
            "this session was not granted the fleet tools, so the fleet-wide harness view is \
             withheld; harness sessions this process is running are still listed"
                .to_string(),
        );
    }
    // `worker.list` probes each worker for its workflow catalogue on the way
    // past, which is wasted here — but it is the only op that reports what a
    // worker is running, and the probe is bounded server-side at two seconds.
    let answer = session
        .fleet
        .call("worker.list", json!({}))
        .await
        .map_err(describe)?;
    serde_json::from_value(answer["workers"].clone())
        .map_err(|err| format!("the fleet answered with a roster this build cannot read: {err}"))
}

/// Why the fleet could not be asked, phrased for the model that will read it.
fn describe(error: ControlError) -> String {
    match error {
        ControlError::NoInstance => {
            "no Medulla fleet is reachable from this session, so what its harnesses are doing \
             cannot be seen from here — harness sessions this process is running are still listed"
                .to_string()
        }
        ControlError::Refused(failure) => failure.message,
        ControlError::Disconnected(reason) | ControlError::Transport(reason) => reason,
    }
}

/// What to say about the run, which is several situations.
///
/// The distinction worth drawing is between "nothing is running because nothing
/// is left to run" and "nothing is running that this host can see", because the
/// second is the one where the absence proves nothing.
pub(super) fn note(standing: Standing, dispatches: usize) -> &'static str {
    match standing {
        Standing::Settled => "this run has finished; its steps below are the whole of what it did",
        Standing::AwaitingApproval => {
            "this run is parked at an approval gate and nothing is executing it — it stays here \
             until it is resumed or rejected, however long that takes"
        }
        _ if dispatches > 0 => {
            "the harness sessions listed are running now; what each one is doing inside its \
             session is not visible from here, only that the worker is still working on it"
        }
        Standing::ExecutingHere => {
            "this run is executing in this process but has no harness session in flight — it is \
             between steps, or on a node kind that runs in-process (a code, transform, or \
             http_request node)"
        }
        Standing::Unaccounted => {
            "no harness session for this run is running anywhere this host can see, and the run \
             is not executing in this process either; it may be running in another process (the \
             TUI, or a daemon), or it may have been interrupted without recording an outcome"
        }
    }
}

/// One run, joined to whatever is being done for it right now.
///
/// The durable half comes from [`ops::get_run`] at whatever step detail the
/// caller asked for, so this is a superset of `workflow_run_get` rather than a
/// competing shape — a caller can switch between the two without relearning the
/// record.
///
/// # Errors
///
/// Returns [`WorkflowError::NotFound`] for a run id the store has never seen.
/// A fleet that cannot be reached is *not* an error; it is reported inside the
/// answer, because the run record is still the thing that was asked for.
pub(crate) async fn detail(
    session: &McpSession,
    run_id: &str,
    steps: StepDetail,
) -> Result<Value, WorkflowError> {
    let record = ops::get_run(&session.store, run_id, steps)?;
    let executing_here = crate::workflows::run::is_running(run_id);
    let standing = Standing::of(&record, executing_here);
    let prefix = dispatch_prefix(run_id);

    // `executingHere` doubles as the answer to "can I stop this?" —
    // `workflow_run_cancel` reaches the same process-local registry — so it is
    // reported once under the name that says what it *is* rather than twice.
    let mut live = json!({
        "executingHere": executing_here,
        "taskIdPrefix": prefix,
    });
    let object = live.as_object_mut().expect("a json object");
    let local = dispatches_here(run_id, &prefix);
    let dispatches = match roster(session).await {
        Ok(workers) => merge(dispatches_in(&workers, &prefix), local),
        Err(reason) => {
            object.insert("fleetUnavailable".to_string(), json!(reason));
            merge(Vec::new(), local)
        }
    };
    let count = dispatches.len();
    object.insert("harnesses".to_string(), json!(dispatches));
    object.insert("note".to_string(), json!(note(standing, count)));

    Ok(json!({ "run": record, "live": live }))
}
