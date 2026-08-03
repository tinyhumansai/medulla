//! Unit tests for the orchestrator hub, split by surface so no file exceeds the
//! repo's 500-line ceiling: [`activity`] covers the in-memory activity ring and
//! its attribution; [`roster`] covers advertising, addressing and dedupe;
//! [`dispatch`] the sender-runner's full dispatch/route/settle path against a
//! fake worker; [`liveness`] the two-layer timeout gate of host-link protocol
//! §6.3.

mod activity;
mod capabilities;
mod dispatch;
mod handoff_advert;
mod held;
mod liveness;
mod roster;
mod system_info;
// The cloud workflow plane is exercised against the real store adapter, which
// only exists when the engine is compiled in.
#[cfg(feature = "workflows")]
mod workflows;
