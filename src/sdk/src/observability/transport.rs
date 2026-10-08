//! Sentry envelope transport over the workspace's own reqwest 0.12 stack.
//!
//! Sentry's bundled transports are the wrong fit here: its `reqwest` transport
//! pins reqwest 0.13 and its TLS features select a rustls crypto provider of
//! their own, while this workspace deliberately runs a single `ring` provider
//! (see the `rustls` note in the workspace `Cargo.toml`). Sending envelopes
//! with the reqwest the rest of Medulla already links keeps one HTTP stack and
//! one TLS backend in the binary.
//!
//! Envelopes are queued on a bounded channel and sent from one dedicated
//! thread that owns a current-thread tokio runtime, so capturing an event never
//! blocks the caller and works identically before, inside, or after the app's
//! own runtime — including from a panic hook on a runtime worker.

use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use sentry::transports::{RateLimiter, RateLimitingCategory};
use sentry::{sentry_debug, ClientOptions, Envelope, Transport};

/// How many envelopes may wait for the sender thread before new ones drop.
const QUEUE_DEPTH: usize = 30;
/// Per-request ceiling, so a dead network cannot wedge the sender thread.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Read the status recorded for this Sentry client, if its latest attempt got
/// a response.
pub(super) fn last_status(status: &AtomicU16) -> Option<u16> {
    match status.load(Ordering::SeqCst) {
        0 => None,
        status => Some(status),
    }
}

/// Work items for the sender thread.
enum Task {
    // Boxed: an envelope is ~450 bytes and every queue slot would pay for it.
    Send(Box<Envelope>),
    Flush(mpsc::SyncSender<()>),
    Shutdown,
}

/// A bounded, non-blocking Sentry transport backed by reqwest 0.12.
pub(super) struct ReqwestTransport {
    sender: mpsc::SyncSender<Task>,
    handle: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
}

impl ReqwestTransport {
    /// Start the sender thread for the DSN in `options`.
    ///
    /// The HTTP client is built on the sender thread itself: a reqwest client
    /// must not be created or dropped on a tokio worker, and this constructor
    /// may run on one.
    fn new(options: &ClientOptions, last_status: Arc<AtomicU16>) -> Self {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_DEPTH);
        let stopping = Arc::new(AtomicBool::new(false));
        let sender_stopping = Arc::clone(&stopping);
        let target = options.dsn.as_ref().map(|dsn| {
            (
                dsn.envelope_api_url().to_string(),
                dsn.to_auth(Some(&options.user_agent)).to_string(),
            )
        });
        let handle = std::thread::Builder::new()
            .name("medulla-sentry".into())
            .spawn(move || {
                if let Some((url, auth)) = target {
                    run_sender(receiver, &url, &auth, last_status.clone(), sender_stopping);
                }
            })
            .ok();
        Self {
            sender,
            handle,
            stopping,
        }
    }
}

/// The sender thread's loop: drain tasks until shutdown or disconnect.
fn run_sender(
    receiver: mpsc::Receiver<Task>,
    url: &str,
    auth: &str,
    last_status: Arc<AtomicU16>,
    stopping: Arc<AtomicBool>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        sentry_debug!("could not start the Sentry transport runtime");
        return;
    };
    // reqwest wires its connector to the ambient reactor at build time.
    let _context = runtime.enter();
    let Ok(client) = reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() else {
        sentry_debug!("could not build the Sentry HTTP client");
        return;
    };
    let mut rate_limiter = RateLimiter::new();
    for task in receiver {
        if stopping.load(Ordering::Acquire) {
            return;
        }
        let envelope = match task {
            Task::Send(envelope) => *envelope,
            Task::Flush(done) => {
                let _ = done.send(());
                continue;
            }
            Task::Shutdown => return,
        };
        if rate_limiter
            .is_disabled(RateLimitingCategory::Any)
            .is_some()
        {
            continue;
        }
        let Some(envelope) = rate_limiter.filter_envelope(envelope) else {
            continue;
        };
        let mut body = Vec::new();
        if envelope.to_writer(&mut body).is_err() {
            continue;
        }
        let request = client
            .post(url)
            .header("X-Sentry-Auth", auth)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-sentry-envelope",
            )
            .body(body)
            .send();
        match runtime.block_on(request) {
            Ok(response) => {
                last_status.store(response.status().as_u16(), Ordering::SeqCst);
                update_rate_limits(&mut rate_limiter, &response);
            }
            Err(error) => {
                last_status.store(0, Ordering::SeqCst);
                sentry_debug!("failed to send Sentry envelope: {error}");
            }
        }
    }
}

/// Honour Sentry's back-pressure headers so a flood of errors cannot keep
/// hammering an ingestion endpoint that already asked us to stop.
fn update_rate_limits(rate_limiter: &mut RateLimiter, response: &reqwest::Response) {
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    if let Some(value) = header("x-sentry-rate-limits") {
        rate_limiter.update_from_sentry_header(value);
    } else if let Some(value) = header("retry-after") {
        rate_limiter.update_from_retry_after(value);
    } else if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        rate_limiter.update_from_429();
    }
}

impl Transport for ReqwestTransport {
    fn send_envelope(&self, envelope: Envelope) {
        if self
            .sender
            .try_send(Task::Send(Box::new(envelope)))
            .is_err()
        {
            sentry_debug!("Sentry envelope dropped: transport queue full");
        }
    }

    /// Wait until every envelope queued before this call has been sent.
    ///
    /// Tasks are processed in order, so the flush marker is answered only after
    /// the sends ahead of it completed (or failed).
    ///
    /// Enqueueing the marker itself is bounded by `timeout`: a plain blocking
    /// `send` on the bounded channel would wait for a free slot regardless of
    /// the caller's deadline, so a full queue behind a slow request could make
    /// this block for roughly the request timeout even when `timeout` asked
    /// for far less. This matters because `flush` runs during Sentry shutdown
    /// and after a panic, where exceeding the caller's deadline can make
    /// Medulla appear hung.
    fn flush(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        loop {
            match self.sender.try_send(Task::Flush(done_tx.clone())) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => return false,
                Err(mpsc::TrySendError::Full(_)) => {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        return false;
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(5)));
                }
            }
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        done_rx.recv_timeout(remaining).is_ok()
    }

    fn shutdown(&self, timeout: Duration) -> bool {
        let flushed = self.flush(timeout);
        self.stopping.store(true, Ordering::Release);
        let _ = self.sender.try_send(Task::Shutdown);
        flushed
    }
}

impl Drop for ReqwestTransport {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.sender.try_send(Task::Shutdown);
        // Shutdown is queued after outstanding work. Joining ensures the
        // thread and its reqwest client cannot outlive this transport.
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Factory for [`ClientOptions::transport`].
pub(super) fn factory(options: &ClientOptions, last_status: Arc<AtomicU16>) -> Arc<dyn Transport> {
    Arc::new(ReqwestTransport::new(options, last_status))
}
