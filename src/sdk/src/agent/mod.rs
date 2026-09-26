//! Medulla's own local agent: a tinyagents harness, a tool surface, and one
//! turn driver.
//!
//! # What this replaced
//!
//! A local agent turn used to be an RPC into an embedded OpenHuman core
//! (`openhuman.inference_agent_chat`), which meant this SDK linked an entire
//! desktop product — its memory engine, channel providers, cron scheduler and
//! Tauri-facing domains — to run a model in a loop with three tools. TinyAgents
//! is the loop on its own; [`tools`] is the part that was always the host's to
//! own anyway (see its module docs).
//!
//! # Layout
//!
//! [`history`] is the per-thread transcript that makes `resume_session_id`
//! mean something, [`tools`] is the tool surface, [`harness`] builds a configured
//! [`tinyagents::runtime::AgentHarness`] from a route and a checkout,
//! and [`turn`] drives exactly one turn, forwarding the harness event stream to
//! its caller rather than folding it — see that module for why the fold belongs
//! with the consumer.

pub mod harness;
pub mod history;
pub mod tools;
pub mod turn;
