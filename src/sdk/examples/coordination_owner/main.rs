//! The orchestrator side of the coordination e2e chain, ported to the host link
//! (`docs/host-link-protocol.md`).
//!
//! Its job is unchanged: send a `medulla-task/1` frame to a worker daemon, wait
//! for a terminal (`reply` / `error` / `capabilities_result`) frame, and print it
//! as one line of JSON on stdout so the shell can assert on the mock-LLM marker.
//! What changed is everything underneath. There is no relay to poll, no mailbox,
//! no directory and no pre-keys — the frame is a message on channel 0 of a link
//! whose pair key was established at enrollment, and the reply comes back on the
//! same link.
//!
//! # Modes
//!
//! - **enroll** (`--enroll`) — mint one orchestrator/host pair: a pair key, two
//!   node ids and two forwarder keys, written as `node.json` into two identity
//!   directories (§7.3). This stands in for the backend's enrollment endpoints
//!   (§7.2) plus the human who carries the pair key to the host (§7.1). The two
//!   **forwarder** keys are printed so the harness can seed the mock forwarder's
//!   node table; the pair key is never printed, because the backend never sees
//!   one and neither does anything that reads this program's output.
//! - **serve** (`--serve <dir>`) — stay up and run one leg per request file
//!   dropped into `<dir>`. This is not a convenience: the link's SSP state lives
//!   in memory, so an endpoint that exits and restarts comes back at state 0
//!   while its peer still holds state *n*, and neither side's diffs apply until
//!   both restart. A long-lived orchestrator process is what the protocol
//!   assumes, so the harness gets one instead of a process per leg.
//! - **one-shot** (neither flag) — connect, run a single leg, print the JSON,
//!   exit. Correct only against a freshly started daemon, for the reason above.
//!
//! # Flags
//!
//! Process-level:
//!
//! - `--state-dir <dir>` — this endpoint's link identity directory (`node.json`).
//!   Required in every mode.
//! - `--forwarder <host:port>` — overrides the forwarder endpoint recorded at
//!   enrollment. **Replaces `--endpoint`**, which named a tiny.place relay base
//!   URL; there is no relay and no HTTP any more.
//! - `--enroll` / `--host-state-dir <dir>` — enrollment mode and the host
//!   identity directory to write.
//! - `--serve <dir>` / `--results <dir>` — serve mode: the request queue, and
//!   where `<label>.json` and `<label>.rc` are written (default: the queue).
//!
//! Per leg (accepted on the command line in one-shot mode, and in a request file
//! in serve mode — one argument per line):
//!
//! - `--to <node-id-hex>` — the worker's **node id** (§2), not an agent handle.
//!   Kept, with a new value space: node names never travel on the wire, so the
//!   only thing that can address a peer here is its id. Checked against the peer
//!   recorded in `node.json`, since a link has exactly one.
//! - `--task <text>` — the task prompt. Unchanged.
//! - `--task-id <id>` — the task/cycle id. Unchanged, and now also the filter
//!   that decides which inbound frame terminates *this* leg.
//! - `--kind <task|capabilities>` — frame kind. Unchanged.
//! - `--provider <opencode|claude|codex>` — provider hint, for the
//!   no-available-provider error path. Unchanged.
//! - `--model <id>` — model hint. Unchanged.
//! - `--timeout-ms <n>` — how long to wait for a terminal frame (default 60000).
//!   Unchanged.
//! - `--reset-link` — serve mode only: rebuild the link before dispatching,
//!   because the peer process restarted (see the serve note above). The harness
//!   knows when it killed a daemon; the link has no in-band way to discover it.
//! - `--reset-only` — rebuild the link and finish, dispatching nothing. Used
//!   between killing a host and starting its replacement, so the frames the old
//!   session was still retransmitting cannot land on the new process and put the
//!   two ends back out of step.
//!
//! Dropped: `--endpoint` (see `--forwarder`), `--publish-only` (there are no
//! pre-keys to publish and no directory to publish them to) and the identity
//! half of `--seed` (identity now comes from `node.json`). `--seed <64hex>`
//! survives in **enroll** mode, where it makes the minted key material
//! deterministic.
//!
//! Exit code: 0 when a reply (or `capabilities_result`) arrived, 1 on an error
//! frame or a timeout, 2 on a usage or transport failure. In serve mode the same
//! code is written to `<label>.rc` per leg and the process itself runs until
//! killed.

mod enroll;
mod leg;
mod serve;

use std::path::PathBuf;
use std::time::Duration;

use medulla::protocol::{HarnessProvider, TaskFrameKind};
use medulla_link::keys::{self, NodeId};

use enroll::enroll;
use leg::{connect, run_leg};
use serve::serve;

/// How often the serve loop looks for new request files.
pub const POLL: Duration = Duration::from_millis(200);

/// What one leg asks for.
#[derive(Debug, Clone)]
pub struct Leg {
    pub to: Option<NodeId>,
    pub task: String,
    pub task_id: String,
    pub kind: TaskFrameKind,
    pub provider: Option<HarnessProvider>,
    pub model: Option<String>,
    pub timeout_ms: u64,
    pub reset_link: bool,
    pub reset_only: bool,
}

impl Default for Leg {
    fn default() -> Self {
        Leg {
            to: None,
            task: "print the coordination marker".to_string(),
            task_id: "coord-1".to_string(),
            kind: TaskFrameKind::Task,
            provider: None,
            model: None,
            timeout_ms: 60_000,
            reset_link: false,
            reset_only: false,
        }
    }
}

/// Everything the process itself needs, plus the leg the command line described.
#[derive(Debug)]
pub struct Args {
    pub state_dir: Option<PathBuf>,
    pub forwarder: Option<String>,
    pub enroll: bool,
    pub host_state_dir: Option<PathBuf>,
    pub serve: Option<PathBuf>,
    pub results: Option<PathBuf>,
    pub seed: Option<String>,
    pub leg: Leg,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            state_dir: std::env::var("MEDULLA_LINK_STATE_DIR")
                .ok()
                .map(PathBuf::from),
            forwarder: std::env::var("MEDULLA_LINK_FORWARDER").ok(),
            enroll: false,
            host_state_dir: None,
            serve: None,
            results: None,
            seed: None,
            leg: Leg::default(),
        }
    }
}

/// Parse one flag stream. Used for argv and, in serve mode, for a request file.
pub fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args::default();
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--state-dir" => args.state_dir = Some(PathBuf::from(value()?)),
            "--forwarder" => args.forwarder = Some(value()?),
            "--enroll" => args.enroll = true,
            "--host-state-dir" => args.host_state_dir = Some(PathBuf::from(value()?)),
            "--serve" => args.serve = Some(PathBuf::from(value()?)),
            "--results" => args.results = Some(PathBuf::from(value()?)),
            "--seed" => args.seed = Some(value()?),
            "--to" => args.leg.to = Some(parse_node_id(&value()?)?),
            "--task" => args.leg.task = value()?,
            "--task-id" => args.leg.task_id = value()?,
            "--model" => args.leg.model = Some(value()?),
            "--reset-link" => args.leg.reset_link = true,
            "--reset-only" => {
                args.leg.reset_link = true;
                args.leg.reset_only = true;
            }
            "--kind" => {
                let raw = value()?;
                args.leg.kind =
                    TaskFrameKind::from_wire(&raw).ok_or(format!("unknown --kind: {raw}"))?;
            }
            "--provider" => {
                let raw = value()?;
                args.leg.provider = Some(
                    HarnessProvider::from_wire(&raw).ok_or(format!("unknown --provider: {raw}"))?,
                );
            }
            "--timeout-ms" => {
                args.leg.timeout_ms = value()?
                    .parse()
                    .map_err(|_| "--timeout-ms must be a number".to_string())?;
            }
            other => return Err(format!("unexpected argument: {other}")),
        }
    }
    Ok(args)
}

/// Decode a 32-character hex node id.
fn parse_node_id(text: &str) -> Result<NodeId, String> {
    let bytes = decode_hex(text.trim(), 16)?;
    Ok(NodeId(bytes.try_into().expect("decode_hex checked length")))
}

/// Decode exactly `want` bytes of hex.
pub fn decode_hex(text: &str, want: usize) -> Result<Vec<u8>, String> {
    if text.len() != want * 2 {
        return Err(format!(
            "expected {} hex characters, got {}",
            want * 2,
            text.len()
        ));
    }
    (0..want)
        .map(|i| {
            u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
                .map_err(|_| format!("not hex: {text:?}"))
        })
        .collect()
}

/// Lowercase hex, the encoding the forwarder's `--node` flag expects.
pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::main]
async fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("coordination_owner: {err}");
            std::process::exit(2);
        }
    };
    let code = match run(args).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("coordination_owner: {err}");
            2
        }
    };
    std::process::exit(code);
}

/// Dispatch on mode, returning the process exit code.
async fn run(args: Args) -> Result<i32, String> {
    let state_dir = args
        .state_dir
        .clone()
        .ok_or("missing --state-dir (or $MEDULLA_LINK_STATE_DIR)")?;
    if args.enroll {
        enroll(&args, &state_dir)?;
        return Ok(0);
    }

    let owner_id = keys::read_node_state(&keys::node_path(&state_dir))
        .map_err(|e| format!("could not read {}: {e}", state_dir.display()))?
        .node_id;
    let link = connect(&state_dir, args.forwarder.as_deref()).await?;

    match args.serve.clone() {
        Some(queue) => {
            let results = args.results.clone().unwrap_or_else(|| queue.clone());
            serve(link, &state_dir, &args, owner_id, &queue, &results).await
        }
        None => {
            let (code, report) = run_leg(&link, owner_id, &args.leg).await;
            println!("{report}");
            Ok(code)
        }
    }
}
