//! Unit tests for the orchestrator hub, split by surface so no file exceeds the
//! repo's 500-line ceiling: [`roster`] covers advertising, addressing and
//! dedupe; [`dispatch`] the sender-runner's full dispatch/route/settle path
//! against a fake worker.

mod dispatch;
mod roster;
