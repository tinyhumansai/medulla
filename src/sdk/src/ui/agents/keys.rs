//! Lane-key parsing: recover the wire `(cycleId, taskId)` from a composed lane key.

/// Split a lane task key into its `(cycleId, taskId)` parts. CoreRuntime composes a
/// lane-unique key `"<cycleId>/t:<taskId>"` (§3.3(2)/§4.4) so two cycles delegating
/// the same bare `taskId` never collide; this recovers the wire ids for steering
/// calls (`task.cancel` / `question.answer`). A key with no `/t:` marker is a bare
/// taskId with no cycle (the mock/backend runtimes), yielding `(None, key)`.
pub fn parse_task_key(key: &str) -> (Option<&str>, &str) {
    match key.split_once("/t:") {
        Some((cycle, task)) => (Some(cycle), task),
        None => (None, key),
    }
}
