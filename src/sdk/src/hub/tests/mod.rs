//! Unit tests for the orchestrator hub, split by surface so no file exceeds the
//! repo's 500-line ceiling: [`activity`] covers the in-memory activity ring and
//! its attribution; [`roster`] covers advertising, addressing and dedupe;
//! [`dispatch`] the sender-runner's full dispatch/route/settle path against a
//! fake worker.

mod activity;
mod dispatch;
mod roster;
