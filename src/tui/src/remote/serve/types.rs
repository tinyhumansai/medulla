//! Types for the remote serving loop.

use std::collections::HashMap;

use crate::worker::stream::SessionStream;

/// Which session a client is currently watching, and the stream feeding it.
///
/// **At most one per client**, and that is a transport constraint rather than a
/// simplification: channel 1 of the host link is a single `RowGrid` per peer, so
/// two concurrent screen streams to one client would overwrite each other's rows
/// on every frame and each switch would resend the whole screen. One subscribed
/// session also matches what the client actually draws — a pane shows one
/// session — and what mosh does.
///
/// Sessions the client is *not* watching stay fully alive and keep appearing in
/// the session list, which rides channel 0. Only the pixels are exclusive.
#[derive(Default)]
pub struct Subscriptions {
    /// The session each client peer is watching, and its frame chain.
    inner: HashMap<String, (String, SessionStream)>,
}

impl Subscriptions {
    /// Point `peer` at `session_id`, replacing whatever it was watching.
    ///
    /// Returns the session it stopped watching, if that changed — the caller
    /// uses it to decide whether anything needs saying in the log.
    pub fn subscribe(&mut self, peer: &str, session_id: &str) -> Option<String> {
        let previous = self.inner.get(peer).map(|(id, _)| id.clone());
        if previous.as_deref() == Some(session_id) {
            return None;
        }
        self.inner.insert(
            peer.to_string(),
            (session_id.to_string(), SessionStream::new(session_id)),
        );
        previous
    }

    /// Stop `peer` watching `session_id`; `false` when it was not watching it.
    pub fn unsubscribe(&mut self, peer: &str, session_id: &str) -> bool {
        let matches = self.inner.get(peer).is_some_and(|(id, _)| id == session_id);
        if matches {
            self.inner.remove(peer);
        }
        matches
    }

    /// Forget everything `peer` was watching.
    pub fn drop_peer(&mut self, peer: &str) {
        self.inner.remove(peer);
    }

    /// Make the next frame for `peer` a full one.
    pub fn request_resync(&mut self, peer: &str) {
        if let Some((_, stream)) = self.inner.get_mut(peer) {
            stream.request_resync();
        }
    }

    /// The session `peer` is watching, if any.
    pub fn session_for(&self, peer: &str) -> Option<&str> {
        self.inner.get(peer).map(|(id, _)| id.as_str())
    }

    /// Every (peer, session, stream) currently subscribed, for one sampling pass.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &String, &mut SessionStream)> {
        self.inner
            .iter_mut()
            .map(|(peer, (id, stream))| (peer, &*id, stream))
    }

    /// How many clients are watching something.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether nobody is watching anything.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
