//! End-to-end coverage for the installed `medulla` command surface.
//!
//! These tests execute the coverage-instrumented binary with isolated homes and
//! offline inputs. Besides checking the public CLI contract, they exercise the
//! process-wiring layer that library-level feature tests intentionally bypass.

use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::FromRawFd;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Wait for a child without allowing a broken subprocess to wedge the suite.
fn wait_with_output_bounded(mut child: std::process::Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    pipe.read_to_end(&mut stdout).expect("read child stdout");
                }
                if let Some(mut pipe) = child.stderr.take() {
                    pipe.read_to_end(&mut stderr).expect("read child stderr");
                }
                return Output {
                    status,
                    stdout,
                    stderr,
                };
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the medulla child did not exit within 10 seconds");
            }
            Err(error) => panic!("failed waiting for medulla child: {error}"),
        }
    }
}

/// Run the workspace binary with a private Medulla home and no inherited
/// credentials or model keys that could make an offline command contact a
/// service.
fn run(args: &[&str], cwd: &std::path::Path, home: &std::path::Path) -> Output {
    run_with_env(args, cwd, home, &[])
}

/// `run`, with `extra` set on the child after the scrubbing below.
///
/// The scrubbing is what makes these tests independent of whoever is running
/// them, so anything a test needs has to be re-added rather than relied upon.
fn run_with_env(
    args: &[&str],
    cwd: &std::path::Path,
    home: &std::path::Path,
    extra: &[(&str, &str)],
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_medulla"));
    command
        .args(args)
        .current_dir(cwd)
        .env("MEDULLA_HOME", home)
        .env("MEDULLA_MCP_ATTACHED", "test-launch")
        .env("MEDULLA_CLAUDE_SESSIONS_DIR", home.join("claude-sessions"))
        .env("MEDULLA_CODEX_SESSIONS_DIR", home.join("codex-sessions"))
        .env_remove("MEDULLA_TOKEN")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("MEDULLA_BACKEND_URL")
        .env_remove("MEDULLA_SENTRY_DSN")
        .env_remove(medulla::analytics::API_URL_ENV);
    // Telemetry is on by default, and with no endpoint override it reports to
    // the live projects, so every child is opted out unless the test points
    // telemetry at its own loopback listener. Then the opt-out is removed, so
    // an opted-out machine cannot turn those success-path tests into no-ops;
    // the opt-out tests set it back through `extra`.
    let local_endpoint = extra
        .iter()
        .any(|(name, _)| *name == "MEDULLA_SENTRY_DSN" || *name == medulla::analytics::API_URL_ENV);
    if local_endpoint {
        // Clearing the opt-out enables both transports, so the one the test
        // does not redirect is aimed at a closed loopback port rather than left
        // on its live default; `extra` overrides either with a real listener.
        command
            .env_remove("MEDULLA_ANALYTICS_DISABLED")
            .env("MEDULLA_SENTRY_DSN", "http://unused@127.0.0.1:9/0")
            .env(medulla::analytics::API_URL_ENV, "http://127.0.0.1:9");
    } else {
        command.env("MEDULLA_ANALYTICS_DISABLED", "1");
    }
    for (name, value) in extra {
        command.env(name, value);
    }
    command.output().expect("the medulla binary should run")
}

#[test]
fn version_help_and_sessions_are_available_without_a_tty() {
    let dir = TempDir::new().unwrap();

    let version = run(&["version"], dir.path(), dir.path());
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("medulla "));

    let help = run(&["help"], dir.path(), dir.path());
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage:"));

    let sessions = run(&["sessions"], dir.path(), dir.path());
    assert!(sessions.status.success());
    assert_eq!(String::from_utf8_lossy(&sessions.stdout).trim(), "[]");
}

#[test]
fn logout_clears_the_session_and_the_retired_credential() {
    // Adoption is a *startup* behaviour: an install predating the store is
    // signed in and should stay signed in. That is exactly why logout must
    // sweep — leaving the file means the next launch verifies the retired
    // bearer and signs the operator straight back in, having just been told
    // they were logged out.
    let dir = TempDir::new().unwrap();
    // Inside the account directory, where a pre-cutover install's file would be
    // read from: `MEDULLA_HOME` names the root that holds accounts.
    let account_home = dir.path().join("local");
    std::fs::create_dir_all(&account_home).unwrap();
    let credentials = account_home.join("credentials.json");
    std::fs::write(
        &credentials,
        r#"{"baseUrl":"http://example","jwt":"secret"}"#,
    )
    .unwrap();
    let session = account_home.join("session.json");
    std::fs::write(
        &session,
        r#"{"token":"jwt-1","userId":"local","baseUrl":"http://example"}"#,
    )
    .unwrap();

    let output = run(&["logout"], dir.path(), dir.path());

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!session.exists(), "the session store is cleared");
    assert!(
        !credentials.exists(),
        "a retired credential must not survive logout — the next launch adopts it"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Logged out"), "{stdout}");
    assert!(stdout.contains("retired credential file"), "{stdout}");
}

/// Logging out when already signed out succeeds, so an operator unsure of their
/// state can always run it.
#[test]
fn logout_is_idempotent_when_signed_out() {
    let dir = TempDir::new().unwrap();
    let output = run(&["logout"], dir.path(), dir.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Logged out"));
}

/// An environment bearer outranks the store, so clearing the store leaves the
/// next process authenticated. Reporting success there tells an operator they
/// revoked access they still hold — the failure logging out exists to prevent.
#[test]
fn logout_refuses_to_claim_success_while_an_environment_token_still_wins() {
    let dir = TempDir::new().unwrap();
    let output = run_with_env(
        &["logout"],
        dir.path(),
        dir.path(),
        &[("MEDULLA_TOKEN", "still-authenticated")],
    );

    assert!(
        !output.status.success(),
        "logout must not report success while a token still authenticates"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MEDULLA_TOKEN"),
        "the message must name the source to remove: {stderr}"
    );
}

#[test]
fn init_offline_writes_then_protects_a_workspace_profile() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "# Example\n\nA small workspace.\n",
    )
    .unwrap();

    let first = run(&["init", ".", "--offline"], dir.path(), dir.path());
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(dir.path().join("MEDULLA.md").exists());
    assert!(String::from_utf8_lossy(&first.stdout).contains("Wrote"));

    let second = run(&["init", ".", "--offline"], dir.path(), dir.path());
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already exists"));

    let forced = run(
        &["init", ".", "--offline", "--force"],
        dir.path(),
        dir.path(),
    );
    assert!(forced.status.success());
}

#[test]
fn invalid_cli_inputs_and_non_tty_tui_exit_cleanly() {
    let dir = TempDir::new().unwrap();

    let bad_login = run(
        &["login", "--provider", "not-a-provider"],
        dir.path(),
        dir.path(),
    );
    assert_eq!(bad_login.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_login.stderr).contains("unknown provider"));

    let tui = run(&[], dir.path(), dir.path());
    assert_eq!(tui.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&tui.stderr).contains("requires an interactive terminal"));
}

#[cfg(unix)]
#[test]
fn interactive_tui_drives_commands_and_quits_on_ctrl_c() {
    let dir = TempDir::new().unwrap();
    let binary = env!("CARGO_BIN_EXE_medulla");
    let (mut master, slave) = open_pty();
    let mut reader = master.try_clone().unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    // The drain publishes into shared storage rather than returning its buffer,
    // so the test never has to join it.
    //
    // Joining would deadlock: the thread only returns once reading the PTY
    // master hits EOF, and EOF only arrives once *every* slave fd is closed. Any
    // process that inherited the slave and outlives the TUI — a leaked child, a
    // grandchild that missed the Ctrl-C — holds it open forever, and the test's
    // own `drop(master)` cannot help because the drain holds its own dup. That
    // left the whole job hanging until the CI runner's (previously absent)
    // timeout fired.
    //
    // The assertions only need the bytes seen so far, so the thread is detached
    // and dies with the process.
    let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let drain_sink = std::sync::Arc::clone(&sink);
    std::thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        let mut announced = false;
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let mut buffered = drain_sink.lock().unwrap();
            buffered.extend_from_slice(&chunk[..n]);
            // The shortcut line heads every frame. The wordmark used to be the
            // sentinel, but it is block art in the signal field now and never
            // reaches the wire as the letters "MEDULLA".
            if !announced && buffered.windows(9).any(|window| window == b"Tab views") {
                announced = true;
                ready_tx.send(()).ok();
            }
        }
    });
    // Snapshot whatever the drain has captured. Callers pause first so bytes
    // written just before exit are in the buffer.
    let captured = {
        let sink = std::sync::Arc::clone(&sink);
        move || {
            std::thread::sleep(Duration::from_millis(100));
            sink.lock().unwrap().clone()
        }
    };

    let mut command = Command::new(binary);
    // Never report a test run to the live telemetry projects.
    command.env("MEDULLA_ANALYTICS_DISABLED", "1");
    command
        .args(["--mock", "--no-alt-screen"])
        .env("MEDULLA_HOME", dir.path())
        .env("MEDULLA_STATE_DIR", dir.path().join("state"))
        .env("MEDULLA_NO_UPDATE_CHECK", "1")
        .env_remove("MEDULLA_TOKEN")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    // SAFETY: this hook only makes the already-installed stdin PTY the child's
    // controlling terminal between `fork` and `exec`; it performs no allocation.
    unsafe {
        command.pre_exec(|| {
            if setsid() < 0 || ioctl(0, tiocsctty(), 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .expect("the TUI should start on a pseudo-terminal");

    // Wait for the first rendered frame before sending Ctrl-C. Instrumented
    // binaries can take longer to start, while an arbitrary sleep races raw-mode
    // setup and turns the byte into a process signal instead of a key event.
    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the TUI should render its first frame");
    // Exercise real event-loop command routing and background responses before
    // quitting: open/close the resume picker, toggle async mode, and visit every
    // top-level tab (which triggers the tab-specific load commands).
    master.write_all(b"/resume\r").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    master.write_all(&[27]).unwrap();
    master.write_all(b"/async on\r").unwrap();
    for _ in 0..6 {
        master.write_all(b"\t").unwrap();
        std::thread::sleep(Duration::from_millis(75));
    }
    master.write_all(&[3]).unwrap();

    for _ in 0..100 {
        if let Some(status) = child.try_wait().unwrap() {
            drop(master);
            let output = captured();
            assert!(
                status.success(),
                "TUI exited with {status:?}: {}",
                String::from_utf8_lossy(&output)
            );
            assert!(String::from_utf8_lossy(&output).contains("Tab views"));
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    child.kill().ok();
    child.wait().ok();
    drop(master);
    let output = captured();
    panic!(
        "interactive TUI did not stop after Ctrl-C: {}",
        String::from_utf8_lossy(&output)
    );
}

/// A small direct `openpty(3)` fixture keeps the test independent of the BSD
/// and util-linux `script` command-line dialects.
#[cfg(unix)]
fn open_pty() -> (std::fs::File, std::fs::File) {
    #[repr(C)]
    struct WinSize {
        rows: u16,
        cols: u16,
        x_pixels: u16,
        y_pixels: u16,
    }

    #[cfg_attr(target_os = "linux", link(name = "util"))]
    unsafe extern "C" {
        fn openpty(
            master: *mut std::os::raw::c_int,
            slave: *mut std::os::raw::c_int,
            name: *mut std::os::raw::c_char,
            termios: *const std::ffi::c_void,
            winsize: *const WinSize,
        ) -> std::os::raw::c_int;
    }

    let mut master = -1;
    let mut slave = -1;
    let size = WinSize {
        rows: 24,
        cols: 80,
        x_pixels: 0,
        y_pixels: 0,
    };
    // SAFETY: `openpty` initializes both owned file descriptors; the remaining
    // pointers are either null or point to the correctly laid-out window size.
    let result = unsafe {
        openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
        )
    };
    assert_eq!(result, 0, "openpty failed");
    // SAFETY: successful `openpty` returned two fresh descriptors whose
    // ownership is transferred exactly once to these `File`s.
    unsafe {
        (
            std::fs::File::from_raw_fd(master),
            std::fs::File::from_raw_fd(slave),
        )
    }
}

/// Drive `medulla mcp` over its stdio transport and collect the replies.
///
/// The MCP stdio transport is the only way this command is ever invoked — an
/// ACP agent spawns it — so this exercises the real contract: newline-delimited
/// JSON-RPC in, the same out, and a clean exit when stdin closes.
fn run_mcp(
    requests: &[serde_json::Value],
    home: &std::path::Path,
) -> (Vec<serde_json::Value>, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_medulla"))
        // Never report a test run to the live telemetry projects.
        .env("MEDULLA_ANALYTICS_DISABLED", "1")
        .arg("mcp")
        .env("MEDULLA_HOME", home)
        .env("MEDULLA_MCP_ATTACHED", "test-launch")
        .env_remove("MEDULLA_TOKEN")
        .env_remove("MEDULLA_MCP_SOCKET")
        .env_remove("MEDULLA_MCP_GRANT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the medulla binary should run");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write a request");
        }
    }
    // Dropping stdin closes the stream, which is how an MCP client ends a
    // session; the server must exit rather than hang on it.
    drop(child.stdin.take());

    let output = wait_with_output_bounded(child);
    let replies = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("replies are JSON"))
        .collect();
    (replies, output.status.success())
}

/// The reply to request `id` (`None` for a frame with no usable id).
///
/// The server answers requests concurrently, so replies may arrive in any
/// order; JSON-RPC pairs them by id, and so must these assertions.
fn reply_with_id(replies: &[serde_json::Value], id: impl Into<Option<i64>>) -> &serde_json::Value {
    let id = id.into().map_or(serde_json::Value::Null, Into::into);
    replies
        .iter()
        .find(|reply| reply["id"] == id)
        .unwrap_or_else(|| panic!("no reply with id {id}: {replies:?}"))
}

#[test]
fn mcp_serves_the_workflow_tools_and_exits_when_stdin_closes() {
    let home = TempDir::new().unwrap();

    let (replies, exited_cleanly) = run_mcp(
        &[
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2024-11-05" },
            }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        ],
        home.path(),
    );

    assert!(exited_cleanly, "closing stdin should end the session");
    assert_eq!(replies.len(), 2, "one reply per request: {replies:?}");
    let initialize = reply_with_id(&replies, 1);
    assert_eq!(initialize["result"]["serverInfo"]["name"], "medulla");
    assert_eq!(initialize["result"]["protocolVersion"], "2024-11-05");

    let names: Vec<&str> = reply_with_id(&replies, 2)["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"workflow_list"), "{names:?}");
    // No grant reached this process, so it has no fleet and must not offer one.
    assert!(
        !names.iter().any(|name| name.starts_with("fleet_")),
        "fleet tools were advertised with no grant: {names:?}"
    );
}

#[test]
fn mcp_rejects_an_ambient_registration_not_attached_by_medulla() {
    let home = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_medulla"))
        // Never report a test run to the live telemetry projects.
        .env("MEDULLA_ANALYTICS_DISABLED", "1")
        .arg("mcp")
        .env("MEDULLA_HOME", home.path())
        .env_remove("MEDULLA_MCP_ATTACHED")
        .stdin(Stdio::null())
        .output()
        .expect("the medulla binary should run");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("only to harnesses explicitly attached by Medulla"),
        "the refusal should identify the missing provenance: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn mcp_answers_a_notification_with_nothing() {
    // A JSON-RPC notification has no id and by definition gets no reply. A
    // server that answered one would desynchronise the client's correlation.
    let home = TempDir::new().unwrap();

    let (replies, exited_cleanly) = run_mcp(
        &[
            serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
        ],
        home.path(),
    );

    assert!(exited_cleanly);
    assert_eq!(
        replies.len(),
        1,
        "the notification drew a reply: {replies:?}"
    );
    assert_eq!(replies[0]["id"], 1);
}

#[test]
fn mcp_answers_a_malformed_frame_and_keeps_going() {
    let home = TempDir::new().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_medulla"))
        // Never report a test run to the live telemetry projects.
        .env("MEDULLA_ANALYTICS_DISABLED", "1")
        .arg("mcp")
        .env("MEDULLA_HOME", home.path())
        .env("MEDULLA_MCP_ATTACHED", "test-launch")
        .env_remove("MEDULLA_TOKEN")
        .env_remove("MEDULLA_MCP_SOCKET")
        .env_remove("MEDULLA_MCP_GRANT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the medulla binary should run");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{{not json}}").unwrap();
        writeln!(
            stdin,
            "{}",
            serde_json::json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" })
        )
        .unwrap();
    }
    drop(child.stdin.take());

    let output = wait_with_output_bounded(child);
    let replies: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    // One bad frame does not end the session: the client that sent it is still
    // a client, and the next request is answered normally.
    assert_eq!(reply_with_id(&replies, None)["error"]["code"], -32700);
    assert!(
        reply_with_id(&replies, 7)["result"].is_object(),
        "{replies:?}"
    );
}

/// Read one HTTP request in full, answer it `200 OK`, and return its body.
fn answer_analytics_request(mut stream: std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let mut request = Vec::new();
    let mut buf = [0u8; 4096];
    let headers_end = loop {
        if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
        let read = stream.read(&mut buf).expect("read headers");
        assert_ne!(read, 0, "connection closed before headers completed");
        request.extend_from_slice(&buf[..read]);
    };
    let content_length: usize = String::from_utf8_lossy(&request[..headers_end])
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::to_owned)
        })
        .and_then(|value| value.trim().parse().ok())
        .expect("content length");
    while request.len() < headers_end + content_length {
        let read = stream.read(&mut buf).expect("read body");
        assert_ne!(read, 0, "connection closed before body completed");
        request.extend_from_slice(&buf[..read]);
    }
    stream
        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        .expect("respond");
    String::from_utf8_lossy(&request[headers_end..headers_end + content_length]).into_owned()
}

/// Accept requests until `stop` is set, answering each `200 OK`, and return
/// every body received.
///
/// Accepting is polled, so the server always stops and can be joined, even
/// when the command under test never connects.
fn collect_requests() -> (
    u16,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::thread::JoinHandle<Vec<String>>,
) {
    use std::sync::atomic::Ordering;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    listener
        .set_nonblocking(true)
        .expect("non-blocking listener");
    let port = listener.local_addr().expect("listener address").port();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopping = std::sync::Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        let mut bodies = Vec::new();
        while !stopping.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).expect("blocking stream");
                    bodies.push(answer_analytics_request(stream));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        bodies
    });
    (port, stop, server)
}

#[test]
fn logout_reports_the_signed_out_account_to_the_configured_analytics_endpoint() {
    let dir = TempDir::new().unwrap();
    let account_home = dir.path().join("local");
    std::fs::create_dir_all(&account_home).unwrap();
    std::fs::write(
        account_home.join("session.json"),
        r#"{"token":"jwt-1","userId":"user-42","baseUrl":"http://example"}"#,
    )
    .unwrap();
    let (port, stop, server) = collect_requests();
    let api_url = format!("http://127.0.0.1:{port}/api");

    let output = run_with_env(
        &["logout"],
        dir.path(),
        dir.path(),
        &[(medulla::analytics::API_URL_ENV, api_url.as_str())],
    );
    stop.store(true, std::sync::atomic::Ordering::Release);
    let bodies = server.join().expect("analytics server");

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(bodies.len(), 1, "analytics requests: {bodies:?}");
    let body: serde_json::Value = serde_json::from_str(&bodies[0]).expect("analytics JSON");
    assert_eq!(body["type"], "track");
    assert_eq!(body["payload"]["name"], "signed_out");
    assert_eq!(body["payload"]["profileId"], "user-42");
}

/// A plain passthrough wrapper session (`--no-bridge`) has no host link to
/// publish to, but its token usage still reaches analytics.
#[cfg(unix)]
#[test]
fn a_bridgeless_wrapper_session_reports_its_token_usage() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let account_home = dir.path().join("local");
    std::fs::create_dir_all(&account_home).unwrap();
    std::fs::write(
        account_home.join("session.json"),
        r#"{"token":"jwt-1","userId":"user-42","baseUrl":"http://example"}"#,
    )
    .unwrap();
    // `run_with_env` points Codex discovery here; `rollout-*.jsonl` is the
    // transcript name it matches.
    let sessions = dir.path().join("codex-sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let rollout = sessions.join("rollout-usage.jsonl");
    // The script is fixed text: the transcript path and both JSON lines reach
    // it through its environment, so no path is ever spliced into shell
    // source, however it is spelled.
    let fake_codex = dir.path().join("codex");
    std::fs::write(
        &fake_codex,
        "#!/bin/sh\n\
         printf '%s\\n' \"$FAKE_CODEX_META\" \"$FAKE_CODEX_USAGE\" >> \"$FAKE_CODEX_ROLLOUT\"\n",
    )
    .unwrap();
    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "session_id": "codex-usage-e2e", "cwd": workspace },
    })
    .to_string();
    let usage_line = serde_json::json!({
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": { "total_token_usage": { "input_tokens": 7, "output_tokens": 3 } },
        },
    })
    .to_string();
    std::fs::set_permissions(&fake_codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (port, stop, server) = collect_requests();
    let api_url = format!("http://127.0.0.1:{port}/api");

    let output = run_with_env(
        &["codex", "--no-bridge"],
        &workspace,
        dir.path(),
        &[
            (medulla::analytics::API_URL_ENV, api_url.as_str()),
            ("MEDULLA_CODEX_BIN", fake_codex.to_str().unwrap()),
            ("FAKE_CODEX_META", meta.as_str()),
            ("FAKE_CODEX_USAGE", usage_line.as_str()),
            ("FAKE_CODEX_ROLLOUT", rollout.to_str().unwrap()),
        ],
    );
    stop.store(true, std::sync::atomic::Ordering::Release);
    let bodies = server.join().expect("analytics server");

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let usage: Vec<serde_json::Value> = bodies
        .iter()
        .map(|body| serde_json::from_str(body).expect("analytics JSON"))
        .filter(|body: &serde_json::Value| body["payload"]["name"] == "token_usage_reported")
        .collect();
    assert_eq!(usage.len(), 1, "token usage events: {bodies:?}");
    assert_eq!(usage[0]["payload"]["profileId"], "user-42");
    let properties = &usage[0]["payload"]["properties"];
    assert_eq!(properties["input_tokens"], "7", "{properties}");
    assert_eq!(properties["output_tokens"], "3", "{properties}");
}

/// A checkout's `.env` cannot redirect telemetry: only the invoking
/// environment's DSN and analytics endpoint are honoured. (The other half —
/// a `.env`-only override is dropped in favour of the built-in endpoint — is
/// not driven here, since it would send to the live project.)
#[test]
fn a_workspace_env_file_cannot_redirect_telemetry_away_from_the_invoker() {
    use std::sync::atomic::Ordering;

    let dir = TempDir::new().unwrap();
    let (planted_port, planted_stop, planted) = collect_requests();
    let (trusted_port, trusted_stop, trusted) = collect_requests();
    std::fs::write(
        dir.path().join(".env"),
        format!(
            "MEDULLA_SENTRY_DSN=http://publickey@127.0.0.1:{planted_port}/7\n\
             {}=http://127.0.0.1:{planted_port}\n",
            medulla::analytics::API_URL_ENV
        ),
    )
    .unwrap();
    let trusted_dsn = format!("http://publickey@127.0.0.1:{trusted_port}/7");
    let trusted_url = format!("http://127.0.0.1:{trusted_port}");

    for command in ["sentry-test", "analytics-test"] {
        let output = run_with_env(
            &[command],
            dir.path(),
            dir.path(),
            &[
                ("MEDULLA_SENTRY_DSN", trusted_dsn.as_str()),
                (medulla::analytics::API_URL_ENV, trusted_url.as_str()),
            ],
        );
        assert!(
            output.status.success(),
            "{command}: stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    planted_stop.store(true, Ordering::Release);
    trusted_stop.store(true, Ordering::Release);
    let planted = planted.join().expect("planted listener");
    let trusted = trusted.join().expect("trusted listener");

    assert!(
        planted.is_empty(),
        "the .env endpoint was contacted: {planted:?}"
    );
    assert_eq!(trusted.len(), 2, "one request per diagnostic: {trusted:?}");
}

/// When an external credential is what authenticates, the stored session's
/// account is not the one signing out, so no `signed_out` is reported for it.
#[test]
fn logout_under_an_external_token_reports_no_sign_out() {
    use std::sync::atomic::Ordering;

    let dir = TempDir::new().unwrap();
    let account_home = dir.path().join("local");
    std::fs::create_dir_all(&account_home).unwrap();
    std::fs::write(
        account_home.join("session.json"),
        r#"{"token":"jwt-1","userId":"user-42","baseUrl":"http://example"}"#,
    )
    .unwrap();
    let (port, stop, server) = collect_requests();
    let api_url = format!("http://127.0.0.1:{port}/api");

    // Whether logout itself succeeds here is covered elsewhere; this pins
    // only what it reports.
    let _ = run_with_env(
        &["logout"],
        dir.path(),
        dir.path(),
        &[
            (medulla::analytics::API_URL_ENV, api_url.as_str()),
            ("MEDULLA_TOKEN", "an-external-credential"),
        ],
    );
    stop.store(true, Ordering::Release);
    let bodies = server.join().expect("analytics server");

    assert!(
        !bodies.iter().any(|body| body.contains("signed_out")),
        "signed_out was reported for the stored account: {bodies:?}"
    );
}

#[test]
fn sentry_test_reaches_a_configured_dsn_end_to_end() {
    let home = TempDir::new().unwrap();
    let (port, stop, server) = collect_requests();
    let dsn = format!("http://publickey@127.0.0.1:{port}/7");

    let output = run_with_env(
        &["sentry-test"],
        home.path(),
        home.path(),
        &[("MEDULLA_SENTRY_DSN", dsn.as_str())],
    );
    stop.store(true, std::sync::atomic::Ordering::Release);
    server.join().expect("sentry server");

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("sentry event id:"), "{stdout}");
    assert!(stdout.contains("HTTP 200"), "{stdout}");
}

#[test]
fn sentry_test_reports_why_when_reporting_is_opted_out() {
    let home = TempDir::new().unwrap();

    let output = run_with_env(
        &["sentry-test"],
        home.path(),
        home.path(),
        &[("MEDULLA_ANALYTICS_DISABLED", "1")],
    );

    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
}

#[test]
fn analytics_test_reports_why_when_analytics_is_opted_out() {
    // The opt-out path verifies the command's inactive status without network
    // access; the successful path is exercised against a local listener below.
    let home = TempDir::new().unwrap();

    let output = run_with_env(
        &["analytics-test"],
        home.path(),
        home.path(),
        &[("MEDULLA_ANALYTICS_DISABLED", "1")],
    );

    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
}

#[test]
fn analytics_test_reaches_a_configured_endpoint_end_to_end() {
    let home = TempDir::new().unwrap();
    let (port, stop, server) = collect_requests();
    let api_url = format!("http://127.0.0.1:{port}");
    let output = run_with_env(
        &["analytics-test"],
        home.path(),
        home.path(),
        &[(medulla::analytics::API_URL_ENV, api_url.as_str())],
    );
    stop.store(true, std::sync::atomic::Ordering::Release);
    server.join().expect("analytics server");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("HTTP 200"));
}

#[cfg(unix)]
unsafe extern "C" {
    fn setsid() -> std::os::raw::c_int;
    fn ioctl(fd: std::os::raw::c_int, request: std::os::raw::c_ulong, ...) -> std::os::raw::c_int;
}

/// Platform request number for `TIOCSCTTY`.
#[cfg(unix)]
const fn tiocsctty() -> std::os::raw::c_ulong {
    #[cfg(target_os = "macos")]
    {
        0x2000_7461
    }
    #[cfg(not(target_os = "macos"))]
    {
        0x540e
    }
}
