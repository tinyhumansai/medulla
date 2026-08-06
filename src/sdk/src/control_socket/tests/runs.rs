//! The run-reporting registry: what a granted session's runs look like to the
//! Medulla drawing them.

use crate::control_socket::runs::{HarnessRunRegistry, HarnessRunStatus, RunReport};

/// A report for `run_id` of `workflow`, with an optional detail line.
fn report(
    run_id: &str,
    workflow: &str,
    status: HarnessRunStatus,
    detail: Option<&str>,
) -> RunReport {
    RunReport {
        run_id: run_id.to_string(),
        workflow_id: workflow.to_string(),
        status,
        detail: detail.map(str::to_string),
        node: None,
    }
}

#[test]
fn repeat_reports_update_one_run_rather_than_stacking_rows() {
    let registry = HarnessRunRegistry::new();
    registry.report(
        "pty-1",
        report(
            "run-1",
            "review",
            HarnessRunStatus::Running,
            Some("started"),
        ),
    );
    registry.report(
        "pty-1",
        report(
            "run-1",
            "review",
            HarnessRunStatus::Running,
            Some("reading src/"),
        ),
    );
    registry.report(
        "pty-1",
        report("run-1", "review", HarnessRunStatus::Succeeded, None),
    );

    let runs = registry.for_session("pty-1");
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].status, HarnessRunStatus::Succeeded);
    // A report with nothing to say moves the status without blanking the last
    // thing that did have something to say.
    assert_eq!(runs[0].detail.as_deref(), Some("reading src/"));
}

#[test]
fn runs_are_kept_per_session_and_forgotten_with_it() {
    let registry = HarnessRunRegistry::new();
    registry.report(
        "pty-1",
        report("run-1", "a", HarnessRunStatus::Running, None),
    );
    registry.report(
        "pty-2",
        report("run-2", "b", HarnessRunStatus::Running, None),
    );

    assert_eq!(registry.for_session("pty-1").len(), 1);
    registry.forget("pty-1");
    assert!(registry.for_session("pty-1").is_empty());
    assert_eq!(registry.for_session("pty-2").len(), 1);
}

#[test]
fn a_chatty_session_drops_settled_runs_before_the_one_still_going() {
    let registry = HarnessRunRegistry::new();
    registry.report(
        "pty-1",
        report("run-live", "a", HarnessRunStatus::Running, None),
    );
    for index in 0..12 {
        registry.report(
            "pty-1",
            report(
                &format!("run-{index}"),
                "a",
                HarnessRunStatus::Succeeded,
                None,
            ),
        );
    }

    let runs = registry.for_session("pty-1");
    assert!(runs.len() <= 8, "{runs:?}");
    assert!(
        runs.iter().any(|run| run.run_id == "run-live"),
        "the executing run is the one the operator is watching: {runs:?}"
    );
}

#[test]
fn an_unknown_status_word_reads_as_still_running() {
    assert_eq!(
        HarnessRunStatus::from_wire("something-new"),
        HarnessRunStatus::Running
    );
    assert_eq!(
        HarnessRunStatus::from_wire("ok"),
        HarnessRunStatus::Succeeded
    );
    assert_eq!(
        HarnessRunStatus::from_wire("interrupted"),
        HarnessRunStatus::Failed
    );
}

#[test]
fn an_active_run_is_only_evicted_when_nothing_settled_can_be_dropped() {
    use crate::control_socket::runs::MAX_RUNS_PER_SESSION;

    let registry = HarnessRunRegistry::new();
    // One settled run first, then a full session's worth of active ones.
    registry.report(
        "pty-1",
        report("run-done", "review", HarnessRunStatus::Succeeded, None),
    );
    for index in 0..MAX_RUNS_PER_SESSION {
        registry.report(
            "pty-1",
            report(
                &format!("run-{index}"),
                "review",
                HarnessRunStatus::Running,
                None,
            ),
        );
    }

    let runs = registry.for_session("pty-1");
    assert_eq!(runs.len(), MAX_RUNS_PER_SESSION);
    // The finished run went first, and every executing one — the ones the
    // operator is actually watching — survived.
    assert!(runs.iter().all(|run| run.run_id != "run-done"));

    // With nothing settled left to drop, the oldest active run gives way.
    registry.report(
        "pty-1",
        report("run-newest", "review", HarnessRunStatus::Running, None),
    );
    let runs = registry.for_session("pty-1");
    assert_eq!(runs.len(), MAX_RUNS_PER_SESSION);
    assert!(runs.iter().all(|run| run.run_id != "run-0"));
    assert!(runs.iter().any(|run| run.run_id == "run-newest"));
}

#[test]
fn a_run_keeps_only_its_newest_frames() {
    use crate::control_socket::runs::MAX_FRAMES_PER_RUN;

    let registry = HarnessRunRegistry::new();
    let overshoot = 5;
    for index in 0..MAX_FRAMES_PER_RUN + overshoot {
        registry.report(
            "pty-1",
            report(
                "run-1",
                "review",
                HarnessRunStatus::Running,
                Some(&format!("frame {index}")),
            ),
        );
    }

    let runs = registry.for_session("pty-1");
    assert_eq!(runs[0].frames.len(), MAX_FRAMES_PER_RUN);
    // The window slid rather than stopped: the oldest frames are gone and the
    // newest one is the last thing reported.
    assert_eq!(runs[0].frames[0].text, format!("frame {overshoot}"));
    assert_eq!(
        runs[0].frames.last().map(|frame| frame.text.as_str()),
        Some(format!("frame {}", MAX_FRAMES_PER_RUN + overshoot - 1).as_str())
    );
}
