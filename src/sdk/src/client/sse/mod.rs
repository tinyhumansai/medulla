//! Server-Sent Events parsing and the reconnecting session event stream.
//!
//! The implementation moved to `tinyhumans_sdk::sse`, which is where the
//! backend's SSE framing and reconnect-with-cursor behaviour is now defined
//! once for every consumer. This module re-exports it so existing paths in this
//! crate keep resolving.

pub use tinyhumans_sdk::sse::{event_stream, SeqDedup, SseFrame, SseParser};
