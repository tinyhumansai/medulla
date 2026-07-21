//! End-to-end coverage for `medulla run`, the non-interactive core-runtime
//! driver (`src/run.rs`). It drives the installed binary against a minimal,
//! in-test `medulla-serve` NDJSON stub over a unix socket — the same shape a
//! docker e2e uses — and asserts the streamed JSON-line transcript: a `ready`
//! line, the folded cycle events, and a terminal `result`. Unix-only, because
//! the core runtime speaks a unix domain socket.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::JoinHandle;

use serde_json::{json, Value};
use tempfile::TempDir;

/// Run the workspace binary with an isolated home and no inherited credentials
/// or model keys, mirroring the `e2e_cli` harness.
fn run(args: &[&str], home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_medulla"))
        .args(args)
        .current_dir(home)
        .env("MEDULLA_HOME", home)
        .env_remove("MEDULLA_TOKEN")
        .env_remove("MEDULLA_CORE_SOCKET")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("MEDULLA_BACKEND_URL")
        .output()
        .expect("the medulla binary should run")
}

/// A short, process-unique socket path (kept well under the ~104-char sun_path
/// cap by living directly in the temp dir).
fn unique_socket() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("mdl-run-{}-{nanos}.sock", std::process::id()))
}

/// Serve exactly one `medulla-serve` connection synchronously: the `ready`
/// banner, then answer `hello`/`subscribe`, stream one full cycle on `instruct`,
/// and ack `stop`. Enough of the protocol for the driver to round-trip one
/// instruction. Returns the accept-loop thread so the test can join it.
fn spawn_serve_stub(path: &Path) -> JoinHandle<()> {
    let listener = UnixListener::bind(path).expect("bind stub serve socket");
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        serve_conn(stream);
    })
}

/// Drive one connection to completion.
fn serve_conn(stream: UnixStream) {
    let mut writer = stream.try_clone().expect("clone stub stream");
    let mut reader = BufReader::new(stream);

    // serve writes the ready banner first (protocol §3).
    send(
        &mut writer,
        &json!({
            "t": "ready", "protocol": 1, "serve": "3.12.0", "sessionId": "agent",
            "capabilities": ["inference", "tools", "subagents"], "error": null
        }),
    );

    let mut seq: u64 = 0;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return, // peer closed
            Ok(_) => {}
        }
        let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if frame.get("t").and_then(Value::as_str) != Some("req") {
            continue;
        }
        let id = frame
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let op = frame.get("op").and_then(Value::as_str).unwrap_or("");
        if serve_op(&mut writer, op, &id, &mut seq) {
            return; // `stop` closes the connection
        }
    }
}

/// Answer one request. Returns `true` when the connection should close.
fn serve_op(writer: &mut UnixStream, op: &str, id: &str, seq: &mut u64) -> bool {
    match op {
        "hello" => {
            send(
                writer,
                &json!({"t":"res","id":id,"ok":true,"result":{
                    "protocol":1,"sessionId":"agent","ports":["inference","tools","subagents"]
                }}),
            );
            false
        }
        "subscribe" => {
            send(
                writer,
                &json!({"t":"res","id":id,"ok":true,"result":{"subscribed":true,"seq":*seq}}),
            );
            false
        }
        "instruct" => {
            send(
                writer,
                &json!({"t":"res","id":id,"ok":true,"result":{
                    "instructionId":"inst-agent-0","cycleId":"cyc:agent:0"
                }}),
            );
            for event in [
                json!({"kind":"cycle_start","instructionId":"inst-agent-0","cycleId":"cyc:agent:0"}),
                json!({"kind":"task_board_changed","task":{
                    "id":"t1","title":"reconcile","status":"active",
                    "createdAt":"0","updatedAt":"0","delegatedTaskIds":[],"notes":[]}}),
                json!({"kind":"cycle_end","instructionId":"inst-agent-0","cycleId":"cyc:agent:0"}),
            ] {
                *seq += 1;
                send(
                    writer,
                    &json!({"t":"event","seq":*seq,"at":0,"event":event}),
                );
            }
            false
        }
        "stop" => {
            send(
                writer,
                &json!({"t":"res","id":id,"ok":true,"result":{"stopped":true}}),
            );
            true
        }
        _ => {
            send(
                writer,
                &json!({"t":"res","id":id,"ok":false,
                    "error":{"code":"unknown_op","message":"unknown op"}}),
            );
            false
        }
    }
}

/// Write one newline-terminated NDJSON frame.
fn send(writer: &mut UnixStream, frame: &Value) {
    let _ = writer.write_all(format!("{frame}\n").as_bytes());
    let _ = writer.flush();
}

#[test]
fn run_streams_one_cycle_from_a_serve_socket() {
    let home = TempDir::new().unwrap();
    let socket = unique_socket();
    let _ = std::fs::remove_file(&socket);
    let stub = spawn_serve_stub(&socket);

    // A non-existent --config keeps load_config on built-in defaults, so the run
    // never touches the user's real configuration.
    let missing_config = home.path().join("none.toml");
    let out = run(
        &[
            "run",
            "--core-socket",
            socket.to_str().unwrap(),
            "--config",
            missing_config.to_str().unwrap(),
            "reconcile",
            "the",
            "world",
        ],
        home.path(),
    );

    let _ = stub.join();
    let _ = std::fs::remove_file(&socket);

    assert!(
        out.status.success(),
        "run should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("each stdout line is JSON"))
        .collect();

    // First a `ready` line naming the attached serve, last a `result` line.
    assert_eq!(lines.first().unwrap()["type"], "ready");
    assert_eq!(lines.first().unwrap()["sessionId"], "agent");
    let result = lines.last().unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["passCount"], 0);

    // The cycle framing streamed through in between.
    let kinds: Vec<String> = lines
        .iter()
        .filter(|l| l["type"] == "event")
        .filter_map(|l| l["event"]["kind"].as_str().map(str::to_string))
        .collect();
    assert!(kinds.iter().any(|k| k == "cycle_start"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "cycle_end"), "{kinds:?}");
}

#[test]
fn run_without_an_instruction_is_a_usage_error() {
    let home = TempDir::new().unwrap();
    // No instruction text: the parser rejects it before any socket work, so this
    // stays fast and offline.
    let out = run(
        &["run", "--core-socket", "/nonexistent/serve.sock"],
        home.path(),
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("instruction"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
