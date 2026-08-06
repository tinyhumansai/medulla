//! Reporting a run back to the Medulla that granted this process its tools.
//!
//! A `workflow_run` tool call executes the whole run inside the MCP subprocess
//! (see [`crate::workflows::local::LocalRun`]). The operator's Medulla is a
//! different process, and until the run record lands on disk it has no idea any
//! of it is happening — so a harness that sets a twenty-minute workflow going
//! looked, from the rail, exactly like a harness sitting idle.
//!
//! This is the other end of [`crate::control_socket::runs`]: the grant the
//! subprocess already holds is used to say what was started, what the harnesses
//! under it are doing, and how it ended.
//!
//! Two properties are load-bearing:
//!
//! **Best effort.** Reporting is a view of the work, never the work. Nothing
//! here can fail a run — a missing grant, a Medulla that has exited, a refused
//! frame — because the caller would then have traded the run for the picture of
//! it.
//!
//! **Coalesced.** A harness emits progress far faster than anyone reads it, and
//! the registry keeps only the latest line per run. So the forwarder sends the
//! most recent frame at a bounded rate and discards what it overtook, rather
//! than queueing thousands of lines that are stale by the time they arrive.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::flow_engine::NodeProgressSink;

/// The least time between two reports for one run.
///
/// Fast enough that a watching rail row looks live, slow enough that a chatty
/// harness cannot turn a progress stream into a control-socket flood.
const MIN_REPORT_INTERVAL: Duration = Duration::from_millis(250);

/// How many progress frames may sit unsent before new ones are dropped.
///
/// The forwarder coalesces, but only once it is awake: while it is awaiting
/// `connect` or `report_run` a chatty harness can enqueue without limit, and
/// the reader would never see those frames anyway. Dropping at the sink is
/// what makes the queue's size a property of this constant rather than of how
/// long a stalled control plane takes to answer. Terminal reports bypass this
/// entirely — there is exactly one per run, and it is the report that matters.
const MAX_PENDING_PROGRESS: usize = 64;

/// The wire word for a settled run's status.
///
/// Mapped here rather than derived from the enum's own serialization: the
/// control plane's vocabulary is deliberately smaller than the engine's, and a
/// run that ended any way other than cleanly is one thing to the rail.
pub fn wire_status(status: crate::workflows::RunStatus) -> &'static str {
    match status {
        crate::workflows::RunStatus::Succeeded => "succeeded",
        crate::workflows::RunStatus::PendingApproval => "awaiting_approval",
        crate::workflows::RunStatus::Running => "running",
        _ => "failed",
    }
}

/// A live reporter for one run.
///
/// Dropping it ends the forwarder; the run itself is untouched.
pub struct RunReporter {
    reports: tokio::sync::mpsc::UnboundedSender<Report>,
    /// Progress frames queued but not yet taken by the forwarder.
    ///
    /// Shared with the forwarder, which decrements as it drains, so the sink
    /// can tell a backlog from a queue that is simply being kept full.
    pending: Arc<AtomicUsize>,
}

/// One thing worth telling the control plane about a run.
struct Report {
    /// The wire status word — see
    /// [`HarnessRunStatus::from_wire`](crate::control_socket::HarnessRunStatus::from_wire).
    status: &'static str,
    /// What just happened, when there is anything to say.
    detail: Option<String>,
    /// The graph node whose harness said it, for a streamed frame.
    node: Option<String>,
    /// Whether this report must not be dropped by coalescing.
    ///
    /// A status change is the whole point of the channel; a progress line is
    /// one of thousands.
    terminal: bool,
}

impl RunReporter {
    /// Start reporting `run_id` of `workflow_id`, if this process can.
    ///
    /// `None` — and a run that behaves identically — whenever there is nothing
    /// to report to: no grant in the environment, no control plane listening,
    /// or a platform without unix sockets. That is the ordinary case for
    /// `medulla workflow run` in a shell.
    pub fn start(
        env: &HashMap<String, String>,
        workflow_id: &str,
        run_id: &str,
    ) -> Option<RunReporter> {
        #[cfg(unix)]
        {
            let (socket, token) = crate::control_socket::grant_from_env(env)?;
            let (reports, mut inbox) = tokio::sync::mpsc::unbounded_channel::<Report>();
            let workflow_id = workflow_id.to_string();
            let run_id = run_id.to_string();
            let pending = Arc::new(AtomicUsize::new(0));
            let drained = pending.clone();
            tokio::spawn(async move {
                let Ok(mut client) =
                    crate::control_socket::ControlClient::connect(&socket, &token).await
                else {
                    // Nothing is listening. Draining the inbox rather than
                    // returning keeps the sink's sends from failing on every
                    // frame for the length of the run.
                    while inbox.recv().await.is_some() {}
                    return;
                };
                while let Some(mut report) = inbox.recv().await {
                    drained.fetch_sub(usize::from(!report.terminal), Ordering::Relaxed);
                    // Take the newest queued report and drop what it overtook:
                    // the reader only ever shows the latest line, so delivering
                    // the ones behind it would cost bandwidth to display
                    // nothing.
                    //
                    // The terminal report is the exception. `settled` and the
                    // per-node progress sink are separate producers, so a frame
                    // a harness emitted on its way out can be queued *after*
                    // the run has ended — and coalescing it over the terminal
                    // report would leave the rail row running forever. Once a
                    // terminal report is in hand, later progress is stale by
                    // definition and is dropped instead.
                    while let Ok(next) = inbox.try_recv() {
                        drained.fetch_sub(usize::from(!next.terminal), Ordering::Relaxed);
                        if report.terminal && !next.terminal {
                            continue;
                        }
                        report = next;
                    }
                    let terminal = report.terminal;
                    if client
                        .report_run(
                            &run_id,
                            &workflow_id,
                            report.status,
                            report.detail.as_deref(),
                            report.node.as_deref(),
                        )
                        .await
                        .is_err()
                    {
                        while inbox.recv().await.is_some() {}
                        return;
                    }
                    if terminal {
                        return;
                    }
                    tokio::time::sleep(MIN_REPORT_INTERVAL).await;
                }
            });
            let reporter = RunReporter { reports, pending };
            reporter.send(Report {
                status: "running",
                detail: Some("started".to_string()),
                node: None,
                terminal: false,
            });
            Some(reporter)
        }
        #[cfg(not(unix))]
        {
            let _ = (env, workflow_id, run_id);
            None
        }
    }

    /// The sink an `agent` node's harness progress is streamed into.
    ///
    /// Each frame is labelled with the node it came from, because "running
    /// tests" means something different under `verify` than under `implement`.
    pub fn progress_sink(&self) -> NodeProgressSink {
        let reports = self.reports.clone();
        let pending = self.pending.clone();
        Arc::new(move |node: &str, frame: &str| {
            let frame = frame.trim();
            if frame.is_empty() {
                return;
            }
            // Dropped rather than queued once the forwarder is this far behind:
            // it shows one line at a time, so a frame with 64 ahead of it will
            // never be displayed, and queueing it would only trade memory for
            // nothing. See `MAX_PENDING_PROGRESS`.
            if pending.load(Ordering::Relaxed) >= MAX_PENDING_PROGRESS {
                return;
            }
            pending.fetch_add(1, Ordering::Relaxed);
            if reports
                .send(Report {
                    status: "running",
                    detail: Some(frame.to_string()),
                    node: Some(node.to_string()),
                    terminal: false,
                })
                .is_err()
            {
                pending.fetch_sub(1, Ordering::Relaxed);
            }
        })
    }

    /// Report that the run settled, with the wire word for how.
    pub fn settled(&self, status: &'static str, detail: Option<String>) {
        self.send(Report {
            status,
            detail,
            node: None,
            terminal: true,
        });
    }

    /// Queue one report, ignoring a forwarder that has already stopped.
    ///
    /// Counts what it queues for [`MAX_PENDING_PROGRESS`], so the forwarder's
    /// matching decrement stays balanced whichever producer sent the report.
    fn send(&self, report: Report) {
        let counted = !report.terminal;
        if counted {
            self.pending.fetch_add(1, Ordering::Relaxed);
        }
        if self.reports.send(report).is_err() && counted {
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
