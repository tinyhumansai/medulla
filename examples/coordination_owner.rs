//! The owner side of the coordination e2e chain: create a fresh owner identity,
//! publish Signal pre-keys, send a `medulla-tinyplace/1` task frame to the worker
//! daemon over the mock Signal server, then drain the encrypted mailbox until a
//! terminal (reply/error) frame comes back — decrypt it and print it as JSON on
//! stdout for `run.sh` to assert the mock-LLM marker on.
//!
//! Every byte of encryption runs in the real vendored `tinyplace` SDK; only the
//! transport server is mocked. This mirrors what `tests/e2e_signal.rs` drives in
//! process, exposed as a runnable so the Docker suite can point it at a live
//! `medulla daemon`.
//!
//! Flags:
//!   --endpoint <url>     tiny.place (mock Signal) base URL. Also `TINYPLACE_ENDPOINT`.
//!   --to <agentId>       the worker daemon's agent id to delegate to (required
//!                        unless `--publish-only`).
//!   --task <text>        the task prompt (default: "print the coordination marker").
//!   --task-id <id>       the task/cycle id (default: "coord-1").
//!   --kind <k>           frame kind to send: `task` (default) or `capabilities`
//!                        (probe the worker; terminates on `capabilities_result`).
//!   --provider <p>       requested provider hint on the frame (`opencode`, `claude`,
//!                        `codex`); used to drive the no-available-provider error path.
//!   --timeout-ms <n>     how long to wait for a terminal frame (default 60000).
//!   --seed <64hex>       a fixed 32-byte identity seed so the owner id is stable
//!                        across invocations (default: a fresh random identity).
//!   --publish-only       just publish the owner's Signal pre-keys and print
//!                        `OWNER_ID=<id>`, then exit — used to seed a known owner
//!                        bundle for the wrapper leg (which then DMs this owner).
//!
//! Exit code: 0 when a reply frame arrived (or `--publish-only` succeeded), 1
//! otherwise (timeout or error frame).

use std::time::{Duration, Instant};

use medulla::daemon::transport::SignalTransport;
use medulla::tinyplace_support::tinyplace::{
    LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions,
};
use medulla::tinyplace_support::{
    decode_task_frame, encode_task_frame, EncodeFrameInput, HarnessProvider, TaskFrame,
    TaskFrameKind,
};

struct Args {
    endpoint: String,
    to: Option<String>,
    task: String,
    task_id: String,
    timeout_ms: u64,
    seed: Option<String>,
    publish_only: bool,
    kind: TaskFrameKind,
    provider: Option<HarnessProvider>,
}

fn parse_args() -> Result<Args, String> {
    let mut endpoint = std::env::var("TINYPLACE_ENDPOINT").ok();
    let mut to = None;
    let mut task = "print the coordination marker".to_string();
    let mut task_id = "coord-1".to_string();
    let mut timeout_ms = 60_000u64;
    let mut seed = None;
    let mut publish_only = false;
    let mut kind = TaskFrameKind::Task;
    let mut provider = None;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--endpoint" => endpoint = it.next(),
            "--to" => to = it.next(),
            "--task" => task = it.next().ok_or("--task needs a value")?,
            "--task-id" => task_id = it.next().ok_or("--task-id needs a value")?,
            "--seed" => seed = it.next(),
            "--publish-only" => publish_only = true,
            "--kind" => {
                let raw = it.next().ok_or("--kind needs a value")?;
                kind = TaskFrameKind::from_wire(&raw)
                    .ok_or_else(|| format!("unknown --kind: {raw}"))?;
            }
            "--provider" => {
                let raw = it.next().ok_or("--provider needs a value")?;
                provider = Some(
                    HarnessProvider::from_wire(&raw)
                        .ok_or_else(|| format!("unknown --provider: {raw}"))?,
                );
            }
            "--timeout-ms" => {
                timeout_ms = it
                    .next()
                    .ok_or("--timeout-ms needs a value")?
                    .parse()
                    .map_err(|_| "--timeout-ms must be a number".to_string())?;
            }
            other => return Err(format!("unexpected argument: {other}")),
        }
    }

    Ok(Args {
        endpoint: endpoint.ok_or("missing --endpoint (or TINYPLACE_ENDPOINT)")?,
        to,
        task,
        task_id,
        timeout_ms,
        seed,
        publish_only,
        kind,
        provider,
    })
}

/// Decode a 64-char hex seed into a [`LocalSigner`], or generate a fresh one.
fn make_signer(seed: Option<&str>) -> Result<LocalSigner, String> {
    match seed {
        Some(hex) => {
            let hex = hex.trim();
            if hex.len() != 64 {
                return Err(format!("--seed must be 64 hex chars, got {}", hex.len()));
            }
            let mut bytes = [0u8; 32];
            for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
                let s = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
                bytes[i] = u8::from_str_radix(s, 16).map_err(|e| format!("bad --seed: {e}"))?;
            }
            LocalSigner::from_seed(&bytes).map_err(|e| e.to_string())
        }
        None => Ok(LocalSigner::generate()),
    }
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("coordination_owner: {err}");
        std::process::exit(2);
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;

    // A fresh owner identity in a private temp dir (removed on exit).
    let dir = std::env::temp_dir().join(format!("medulla-coord-owner-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let signer = std::sync::Arc::new(make_signer(args.seed.as_deref())?);
    let owner_id = signer.agent_id();
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: args.endpoint.clone(),
        signer: Some(signer.clone() as std::sync::Arc<dyn Signer>),
        ..Default::default()
    });
    let transport = SignalTransport::new(client, &signer, &dir);

    // Publish keys so a peer can open an encrypted channel back to us.
    transport
        .publish_keys(&signer)
        .await
        .map_err(|e| format!("owner publish_keys failed: {e}"))?;

    if args.publish_only {
        // Seed a known owner bundle for the wrapper leg, then exit.
        println!("OWNER_ID={owner_id}");
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(());
    }

    let to = args.to.clone().ok_or("missing --to <worker agent id>")?;
    eprintln!("coordination_owner: owner={owner_id} → worker={to}");

    // Send the request frame (encrypted; opens the X3DH session to the worker).
    // `--kind capabilities` probes the worker instead of delegating a task.
    let frame = encode_task_frame(EncodeFrameInput {
        kind: args.kind,
        task_id: args.task_id.clone(),
        text: args.task.clone(),
        ts: "2026-07-18T00:00:00.000Z".to_string(),
        correlation_id: Some(format!("{}-corr", args.task_id)),
        harness: None,
        provider: args.provider,
    });
    transport
        .send(&to, &frame)
        .await
        .map_err(|e| format!("owner send failed: {e}"))?;
    eprintln!("coordination_owner: task frame sent, draining for reply…");

    // Drain until a terminal frame arrives or the timeout elapses.
    let deadline = Instant::now() + Duration::from_millis(args.timeout_ms);
    let mut collected: Vec<TaskFrame> = Vec::new();
    let mut terminal: Option<TaskFrame> = None;
    while Instant::now() < deadline && terminal.is_none() {
        for message in transport.drain_inbox(50).await {
            if let Some(frame) = decode_task_frame(&message.text) {
                // Reply/Error terminate a task; CapabilitiesResult terminates a
                // capabilities probe.
                let is_terminal = matches!(
                    frame.kind,
                    TaskFrameKind::Reply | TaskFrameKind::Error | TaskFrameKind::CapabilitiesResult
                );
                eprintln!(
                    "coordination_owner: frame kind={:?} text={:?}",
                    frame.kind, frame.text
                );
                collected.push(frame.clone());
                if is_terminal {
                    terminal = Some(frame);
                    break;
                }
            }
        }
        if terminal.is_none() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    let _ = std::fs::remove_dir_all(&dir);

    match terminal {
        Some(frame) => {
            // Emit the terminal frame as JSON on stdout for run.sh to assert on.
            let out = serde_json::json!({
                "kind": format!("{:?}", frame.kind),
                "text": frame.text,
                "taskId": frame.task_id,
                "correlationId": frame.correlation_id,
                "harness": frame.harness.map(|h| h.as_str().to_string()),
                "ownerId": owner_id,
                "frames": collected.len(),
                "frameKinds": collected
                    .iter()
                    .map(|f| f.kind.as_str().to_string())
                    .collect::<Vec<_>>(),
                "usage": frame.usage.map(|u| serde_json::json!({
                    "inputTokens": u.input_tokens,
                    "outputTokens": u.output_tokens,
                })),
            });
            println!("{out}");
            // Reply / CapabilitiesResult are success; Error is failure.
            if matches!(
                frame.kind,
                TaskFrameKind::Reply | TaskFrameKind::CapabilitiesResult
            ) {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
        None => {
            eprintln!(
                "coordination_owner: timed out with no terminal frame ({} frames seen)",
                collected.len()
            );
            std::process::exit(1);
        }
    }
}
