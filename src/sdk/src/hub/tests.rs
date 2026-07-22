//! Unit tests for the sender-runner, driven by a `FakeWorker` [`Relay`] that
//! replays the daemon's `ack → status → reply|error` sequence into the inbox so
//! the runner exercises its full dispatch/route/settle path with no network.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::daemon::transport::InboundMessage;
use crate::tinyplace::{
    decode_task_frame, encode_task_frame_with_usage, EncodeFrameInput, TaskFrameKind, TokenUsage,
};

use super::relay::Relay;
use super::roster::{address_of, register_payload, HubWorker};
use super::types::{RunError, TaskRequest};
use super::TaskRunner;

fn worker(id: &str, addr: &str) -> HubWorker {
    HubWorker {
        id: id.to_string(),
        address: addr.to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
    }
}

#[test]
fn register_payload_advertises_id_address_and_harness() {
    let payload = register_payload(&[worker("w1", "GRVaddr")]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], "w1");
    assert_eq!(agents[0]["metadata"]["address"], "GRVaddr");
    assert_eq!(agents[0]["metadata"]["harness"], "claude");
}

#[test]
fn an_absent_agent_id_falls_back_but_an_unknown_one_does_not() {
    // These were one case and are two. An absent id means "any worker" — the
    // backend omits it for an unattributed task. An id that is present but
    // unrecognised means something addressed a specific agent this hub does not
    // have, and running that on whichever worker sorted first is a wrong answer
    // dressed as a right one.
    let workers = [worker("w1", "ADDR1"), worker("w2", "ADDR2")];
    assert_eq!(address_of(&workers, "w2").as_deref(), Some("ADDR2"));
    assert_eq!(address_of(&workers, "").as_deref(), Some("ADDR1"));
    assert_eq!(address_of(&workers, "   ").as_deref(), Some("ADDR1"));
    assert_eq!(
        address_of(&workers, "unknown"),
        None,
        "an unrecognised target must be refused, not guessed at"
    );
    assert_eq!(address_of(&[], "w1"), None);
}

#[test]
fn a_worker_is_addressable_by_its_cryptoid_too() {
    // A roster saved before ids were human-scale stored the cryptoId *as* the
    // id, and `MEDULLA_HUB_WORKERS` can still pin one. Both must keep resolving
    // or an upgrade silently unaddresses every existing worker.
    let workers = [worker("claude-worker", "3Hob1FxUwsy")];
    assert_eq!(
        address_of(&workers, "3Hob1FxUwsy").as_deref(),
        Some("3Hob1FxUwsy")
    );
    assert_eq!(
        address_of(&workers, "claude-worker").as_deref(),
        Some("3Hob1FxUwsy")
    );
}

#[test]
fn an_advertised_worker_is_online_so_it_can_be_auto_assigned() {
    // The orchestrator auto-assigns an untargeted task only to an agent whose
    // availability is exactly "online". Advertising a blank one excluded this
    // hub's workers from every fan-out, and rendered as an empty column in
    // agent_list — which reads as a broken row, not an idle worker.
    let payload = register_payload(&[worker("w1", "GRVaddr")]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["availability"], "online");
}

#[test]
fn a_worker_id_is_short_stable_and_unique() {
    use super::roster::worker_id;
    // The id is what the orchestrator must reproduce to address the worker; a
    // 44-character base58 cryptoId reads as noise beside a memorable name, and
    // the model reaches for the name.
    assert_eq!(worker_id(None, "claude", &[]), "claude-worker");
    assert_eq!(
        worker_id(Some("Sanil Laptop"), "claude", &[]),
        "sanil-laptop"
    );
    assert_eq!(worker_id(Some("  "), "codex", &[]), "codex-worker");
    // Distinct even when two unlabelled workers share a harness — otherwise one
    // shadows the other in the backend registry.
    let taken = vec!["claude-worker".to_string()];
    assert_eq!(worker_id(None, "claude", &taken), "claude-worker-2");
    // Nothing usable in the label falls back rather than producing an empty id.
    assert_eq!(worker_id(Some("!!!"), "claude", &[]), "claude-worker");
}

#[test]
fn address_of_prefers_the_selected_worker_over_the_first() {
    let mut selected = worker("w2", "ADDR2");
    selected.selected = true;
    let workers = [worker("w1", "ADDR1"), selected];
    // An explicit match still wins.
    assert_eq!(address_of(&workers, "w1").as_deref(), Some("ADDR1"));
    // An ABSENT agentId routes to the SELECTED worker, which is what makes
    // `select()` a real dispatch control rather than a display flag.
    assert_eq!(address_of(&workers, "").as_deref(), Some("ADDR2"));
    // An unrecognised one is refused even with a selection: "any worker" and
    // "that worker, which I do not have" are different requests.
    assert_eq!(address_of(&workers, "unknown"), None);
}

/// How the fake worker responds to a dispatched task.
enum Mode {
    Reply(String),
    Error(String),
    Silent,
    /// Acks (a sign of life) and streams a status, but never sends a terminal
    /// frame — exercises the "peer alive, then the reply times out" path.
    AckOnly,
    /// Silent until the sender has reset the session (simulating a restarted peer
    /// whose first `CIPHERTEXT` is undecryptable), then replies.
    RecoverAfterReset(String),
}

/// A fake worker: on `send`, decodes the task frame and queues the daemon's
/// `ack → status → (reply|error)` sequence (echoing `correlationId`), which the
/// pump then drains. `Silent` queues nothing, to exercise the timeout path.
struct FakeWorker {
    inbox: Mutex<VecDeque<InboundMessage>>,
    mode: Mode,
    /// How many times the sender has reset the session with us.
    resets: AtomicU32,
    /// When true, every `send` fails — exercises the transport-error path.
    fail_send: bool,
    /// `contact_accepted` returns false until it has been polled this many times,
    /// simulating a peer whose auto-accepter settles a few polls later.
    accept_after: u32,
    /// How many times `contact_accepted` has been polled.
    contact_checks: AtomicU32,
}

impl FakeWorker {
    fn new(mode: Mode) -> Arc<Self> {
        Self::with(mode, false, 0)
    }

    /// A worker with explicit send-failure and contact-acceptance-delay knobs.
    fn with(mode: Mode, fail_send: bool, accept_after: u32) -> Arc<Self> {
        Arc::new(Self {
            inbox: Mutex::new(VecDeque::new()),
            mode,
            resets: AtomicU32::new(0),
            fail_send,
            accept_after,
            contact_checks: AtomicU32::new(0),
        })
    }
}

#[async_trait]
impl Relay for FakeWorker {
    async fn send(&self, _to: &str, body: &str) -> Result<(), String> {
        if self.fail_send {
            return Err("send boom".to_string());
        }
        let frame = decode_task_frame(body).expect("runner sends a valid task frame");
        assert_eq!(frame.kind, TaskFrameKind::Task);
        // Stay silent while there's nothing to say: unconditionally for `Silent`,
        // and until a reset has happened for `RecoverAfterReset`.
        let silent = matches!(self.mode, Mode::Silent)
            || matches!(self.mode, Mode::RecoverAfterReset(_))
                && self.resets.load(Ordering::Relaxed) == 0;
        if silent {
            return Ok(());
        }
        let cid = frame.correlation_id.clone();
        let task_id = frame.task_id.clone();
        let mk = |kind, text: &str, usage| InboundMessage {
            from: "worker".to_string(),
            text: encode_task_frame_with_usage(
                EncodeFrameInput {
                    kind,
                    task_id: task_id.clone(),
                    text: text.to_string(),
                    ts: "T".to_string(),
                    correlation_id: cid.clone(),
                    harness: None,
                    provider: None,
                    model: None,
                },
                usage,
            ),
        };
        let mut q = self.inbox.lock().await;
        q.push_back(mk(TaskFrameKind::Ack, "accepted", None));
        q.push_back(mk(TaskFrameKind::Status, "running python audit.py", None));
        match &self.mode {
            Mode::Reply(text) | Mode::RecoverAfterReset(text) => q.push_back(mk(
                TaskFrameKind::Reply,
                text,
                Some(TokenUsage {
                    input_tokens: 3,
                    output_tokens: 5,
                }),
            )),
            Mode::Error(text) => q.push_back(mk(TaskFrameKind::Error, text, None)),
            // Ack + status already queued above; no terminal frame follows.
            Mode::Silent | Mode::AckOnly => {}
        }
        Ok(())
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        let mut q = self.inbox.lock().await;
        let mut out = Vec::new();
        while out.len() < limit as usize {
            match q.pop_front() {
                Some(m) => out.push(m),
                None => break,
            }
        }
        out
    }

    async fn request_contact(&self, _peer: &str) -> Result<(), String> {
        Ok(())
    }

    /// Accepted once polled `accept_after` times (0 → already a contact, so `run`
    /// proceeds straight to the send).
    async fn contact_accepted(&self, _peer: &str) -> bool {
        self.contact_checks.fetch_add(1, Ordering::Relaxed) >= self.accept_after
    }

    async fn reset_session(&self, _peer: &str) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }
}

fn req(instruction: &str) -> TaskRequest {
    TaskRequest {
        task_id: "t1".to_string(),
        cycle_id: Some("c1".to_string()),
        instruction: instruction.to_string(),
        worker_address: "GRV1worker".to_string(),
        provider: None,
        model: None,
        timeout: Duration::from_secs(2),
    }
}

#[tokio::test]
async fn dispatches_and_returns_the_worker_reply_with_usage() {
    let worker = FakeWorker::new(Mode::Reply("REMOTE: 4 agents, 1 offline".to_string()));
    let runner = TaskRunner::start(worker, Duration::from_millis(5));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let outcome = runner.run(req("audit"), Some(tx)).await.expect("ok");

    assert_eq!(outcome.reply, "REMOTE: 4 agents, 1 offline");
    assert_eq!(outcome.usage.input_tokens, 3);
    assert_eq!(outcome.usage.output_tokens, 5);
    // The status frame was forwarded to the progress sink.
    assert_eq!(rx.recv().await.as_deref(), Some("running python audit.py"));
}

#[tokio::test]
async fn surfaces_a_worker_error_frame() {
    let worker = FakeWorker::new(Mode::Error("boom".to_string()));
    let runner = TaskRunner::start(worker, Duration::from_millis(5));

    let err = runner.run(req("x"), None).await.expect_err("errors");
    assert_eq!(err, RunError::Worker("boom".to_string()));
}

#[tokio::test]
async fn times_out_when_the_worker_never_replies() {
    // A permanently-silent peer: the runner resets + resends up to the cap, then
    // gives up with Timeout. Short ack window so the whole retry loop is quick.
    let worker = FakeWorker::new(Mode::Silent);
    let runner = TaskRunner::start_with_ack_window(
        worker,
        Duration::from_millis(5),
        Duration::from_millis(40),
    );
    let err = runner.run(req("x"), None).await.expect_err("times out");
    assert_eq!(err, RunError::Timeout);
}

#[tokio::test]
async fn recovers_from_a_desynced_peer_by_resetting_and_resending() {
    // The peer is silent on the first send (its post-restart CIPHERTEXT would be
    // undecryptable), then answers once the session has been reset. The runner
    // should reset + resend within the ack window and succeed.
    let worker = FakeWorker::new(Mode::RecoverAfterReset("recovered".to_string()));
    let runner = TaskRunner::start_with_ack_window(
        worker.clone(),
        Duration::from_millis(5),
        Duration::from_millis(120),
    );
    let outcome = runner.run(req("x"), None).await.expect("recovers");
    assert_eq!(outcome.reply, "recovered");
    assert!(
        worker.resets.load(Ordering::Relaxed) >= 1,
        "the runner should have reset the session before resending"
    );
}

#[tokio::test]
async fn concurrent_dispatches_are_routed_by_correlation_id() {
    // Two tasks in flight at once must each get their own reply, proving the
    // shared pump fans frames out by correlationId rather than crossing wires.
    let worker = FakeWorker::new(Mode::Reply("done".to_string()));
    let runner = Arc::new(TaskRunner::start(worker, Duration::from_millis(5)));

    let a = {
        let r = runner.clone();
        let mut req_a = req("a");
        req_a.task_id = "ta".to_string();
        tokio::spawn(async move { r.run(req_a, None).await })
    };
    let b = {
        let r = runner.clone();
        let mut req_b = req("b");
        req_b.task_id = "tb".to_string();
        tokio::spawn(async move { r.run(req_b, None).await })
    };
    assert_eq!(a.await.unwrap().unwrap().reply, "done");
    assert_eq!(b.await.unwrap().unwrap().reply, "done");
}

#[tokio::test]
async fn surfaces_a_transport_error_when_the_send_fails() {
    // The relay's send fails outright (e.g. the address can't be decoded): the
    // runner drops the waiter and returns a Transport error, not a hang.
    let worker = FakeWorker::with(Mode::Reply("unused".to_string()), true, 0);
    let runner = TaskRunner::start(worker, Duration::from_millis(5));

    let err = runner.run(req("x"), None).await.expect_err("send fails");
    assert_eq!(err, RunError::Transport("send boom".to_string()));
}

#[tokio::test]
async fn waits_for_contact_acceptance_before_dispatching() {
    // The peer isn't a contact yet: the runner requests one and polls until the
    // auto-accepter settles (here, on the third check) before it sends the task.
    let worker = FakeWorker::with(Mode::Reply("hi".to_string()), false, 2);
    let runner = TaskRunner::start(worker.clone(), Duration::from_millis(5));

    let outcome = runner
        .run(req("x"), None)
        .await
        .expect("dispatches once accepted");
    assert_eq!(outcome.reply, "hi");
    assert!(
        worker.contact_checks.load(Ordering::Relaxed) >= 2,
        "the runner should have polled contact status until acceptance"
    );
}

#[tokio::test]
async fn times_out_after_the_peer_acks_but_never_replies() {
    // The peer answers (ack + status) — so the runner commits to the full request
    // timeout rather than resetting — but never sends a terminal frame, so the
    // dispatch resolves as Timeout once that deadline passes.
    let worker = FakeWorker::new(Mode::AckOnly);
    let runner = TaskRunner::start(worker, Duration::from_millis(5));

    let mut request = req("x");
    request.timeout = Duration::from_millis(80);
    let err = runner
        .run(request, None)
        .await
        .expect_err("times out after ack");
    assert_eq!(err, RunError::Timeout);
}

#[test]
fn run_error_display_is_human_readable_per_variant() {
    assert_eq!(RunError::Timeout.to_string(), "tiny.place task timed out");
    assert_eq!(
        RunError::Worker("boom".to_string()).to_string(),
        "worker error: boom"
    );
    assert_eq!(
        RunError::Transport("no route".to_string()).to_string(),
        "transport error: no route"
    );
}

// ------------------------------------------------------- contact on add ---

/// Adding a peer opens the contact edge; re-adding retries it.
///
/// Deferring the request to first dispatch — which is what used to happen —
/// makes adding a worker look like nothing happened: the peer's operator sees no
/// approval to give, and when one finally appears it is attached to a task
/// already blocked on it. Re-adding is then the natural way to retry a request
/// the peer missed.
///
/// The decision is tested here; the socket plumbing around it belongs to the
/// live staging E2E, as the rest of `HubHandle` does.
#[test]
fn adding_a_peer_requests_contact_unless_it_is_already_one() {
    use super::handle::should_request_contact;

    assert!(
        should_request_contact("peer-address", false),
        "a new peer must be asked"
    );
    assert!(
        should_request_contact("peer-address", false),
        "and a duplicate re-asked, which is how a missed request is retried"
    );
    assert!(
        !should_request_contact("peer-address", true),
        "an accepted contact has nothing left to ask for"
    );
    assert!(
        !should_request_contact("", false),
        "a worker with no address has nobody to ask"
    );
}

// ------------------------------------------------------------- roster dedupe ---

fn hw(id: &str, address: &str) -> HubWorker {
    HubWorker {
        id: id.to_string(),
        address: address.to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
    }
}

#[test]
fn one_peer_never_occupies_two_roster_slots() {
    use super::roster::remove_conflicting;

    // `MEDULLA_HUB_WORKERS="alpha=<addr>"` seeds the id `alpha`; adding the same
    // address in the TUI uses the address as the id. Same wallet, two names.
    let mut roster = vec![hw("alpha", "So1anaAddr")];
    let incoming = hw("So1anaAddr", "So1anaAddr");
    remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(roster.len(), 1, "one destination, one entry");
    assert_eq!(roster[0].id, "So1anaAddr", "the newest naming wins");
}

#[test]
fn re_adding_the_same_id_still_replaces() {
    use super::roster::remove_conflicting;

    let mut roster = vec![hw("w1", "addr-a")];
    let incoming = hw("w1", "addr-b");
    remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].address, "addr-b", "an id can be repointed");
}

#[test]
fn distinct_peers_are_left_alone() {
    use super::roster::remove_conflicting;

    let mut roster = vec![hw("w1", "addr-a"), hw("w2", "addr-b")];
    let incoming = hw("w3", "addr-c");
    remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(roster.len(), 3, "deduping must not collapse real peers");
}

#[test]
fn blank_addresses_do_not_collide_with_each_other() {
    // Two entries with no address are not "the same peer"; collapsing them would
    // silently delete a roster row on an unrelated add.
    use super::roster::remove_conflicting;

    let mut roster = vec![hw("w1", "")];
    let incoming = hw("w2", "");
    remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(roster.len(), 2);
}

#[test]
fn a_handle_is_recognised_as_an_alias_not_an_address() {
    use super::handle::is_handle;

    // Contacts, pre-key bundles and DMs are all keyed on the cryptoId; an
    // `@handle` is only a directory alias, and passing it through unresolved
    // produces `POST /contacts/%40name`, which cannot match anything.
    assert!(is_handle("@alice"));
    assert!(is_handle("  @alice"), "leading space is still a handle");
    assert!(
        !is_handle("8m6ZTfUGMdnoWanb1V31SZncBfr9xA1oAXnkv4cAAHVB"),
        "a cryptoId is already the key"
    );
    assert!(!is_handle(""));
}

#[test]
fn an_implausible_address_is_refused_before_it_reaches_the_relay() {
    use super::handle::is_plausible_address;

    // A stray `>` was accepted as a worker address, registered in the roster,
    // and had a contact request sent to it. Nothing downstream can tell that
    // from a real peer that simply never replies.
    assert!(!is_plausible_address(">"));
    assert!(!is_plausible_address(""));
    assert!(!is_plausible_address("   "));
    assert!(!is_plausible_address("too-short"));
    assert!(
        !is_plausible_address("3Hob1FxUwsy1K2rweppbmCkuPef6unAr5Amj6kQ2fM0A"),
        "base58 excludes 0, O, I and l because they are easy to confuse"
    );

    // Real values must still pass.
    assert!(is_plausible_address(
        "3Hob1FxUwsy1K2rweppbmCkuPef6unAr5Amj6kQ2fM3A"
    ));
    assert!(is_plausible_address(
        "8m6ZTfUGMdnoWanb1V31SZncBfr9xA1oAXnkv4cAAHVB"
    ));
    assert!(is_plausible_address("@alice"));
    assert!(!is_plausible_address("@"), "a bare @ names nobody");
}

#[tokio::test]
async fn every_inbound_worker_frame_is_narrated_with_its_payload() {
    // The hub reported only the settled outcome, so "the worker never answered"
    // and "the worker answered with nothing" read identically from the
    // orchestrator's side — and neither said whether the worker had been
    // talking at all. Every frame it sends is now on the record.
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let worker = FakeWorker::new(Mode::Reply("REMOTE: 4 agents, 1 offline".to_string()));
    let runner = TaskRunner::start_with_log(worker, Duration::from_millis(5), {
        let seen = seen.clone();
        Arc::new(move |line: &str| seen.lock().unwrap().push(line.to_string()))
    });

    let outcome = runner.run(req("audit"), None).await.expect("ok");
    assert_eq!(outcome.reply, "REMOTE: 4 agents, 1 offline");

    let lines = seen.lock().unwrap().clone();
    let reply_line = lines
        .iter()
        .find(|l| l.contains("reply"))
        .unwrap_or_else(|| panic!("the reply must be narrated, got {lines:?}"));
    assert!(
        reply_line.contains("REMOTE: 4 agents, 1 offline"),
        "the payload itself is the point: {reply_line}"
    );
    assert!(
        reply_line.contains("27 chars"),
        "a truncated preview cannot report its own truncation: {reply_line}"
    );
    // The status frame that preceded it is on the record too, so a worker that
    // is working but not finishing is distinguishable from a silent one.
    assert!(
        lines.iter().any(|l| l.contains("running python audit.py")),
        "got {lines:?}"
    );
}

#[test]
fn a_preview_is_clipped_and_flattened_but_says_so() {
    use crate::logging::{preview, PREVIEW_CHARS};

    assert_eq!(preview("one\ntwo   three"), "one two three");
    let long = "x".repeat(PREVIEW_CHARS + 50);
    let out = preview(&long);
    assert!(out.ends_with('…'), "truncation must be visible: {out}");
    assert_eq!(out.chars().count(), PREVIEW_CHARS + 1);
    assert_eq!(preview(""), "");
}

#[test]
fn an_unlabelled_worker_advertises_one_token_not_two() {
    // `agent_list` renders `id (name)`. When those differ and both read as
    // names, the model picks one and may pick the unroutable one — which is the
    // original bug. Unlabelled, they must coincide.
    let payload = register_payload(&[worker("claude-worker", "3Hob1Fxu")]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["id"], "claude-worker");
    assert_eq!(
        agents[0]["name"], "claude-worker",
        "an unlabelled worker must not advertise a second, different name"
    );

    // A labelled one keeps its human name; the id stays a visible slug of it.
    let mut labelled = worker("sanil-laptop", "3Hob1Fxu");
    labelled.label = Some("Sanil Laptop".to_string());
    let payload = register_payload(&[labelled]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["id"], "sanil-laptop");
    assert_eq!(agents[0]["name"], "Sanil Laptop");
}
