//! Running an installed workflow in answer to a task frame.
//!
//! An ordinary task frame carries an instruction and this worker hands it to a
//! harness. A frame naming a `workflow` carries an *id*, and this worker runs
//! the saved graph instead — dispatching each of its `agent` nodes to its own
//! harness, in the order and with the parallelism the graph declares.
//!
//! That is the whole of "the orchestrator can execute workflows": one extra
//! field on a frame it already sends, over the transport it already uses. The
//! admission check, the ack, and the reply are the same ones an ordinary task
//! gets, so an orchestrator that knows nothing about workflows still sees a
//! task it dispatched and a task that answered.
//!
//! Split by responsibility to stay under the repo's 500-line ceiling:
//! [`dispatch`] runs each `agent` node — the provider/transport resolution and
//! transcript collection of [`RuntimeDispatch`] — and [`handle`] runs the
//! frame-level lifecycle: admission, validation, the reply, and the failure
//! review that follows a failed run.

mod dispatch;
mod handle;

pub(in crate::daemon) use dispatch::RuntimeDispatch;
