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
use std::time::Duration;

use tempfile::TempDir;

/// Run the workspace binary with a private Medulla home and no inherited
/// credentials or model keys that could make an offline command contact a
/// service.
fn run(args: &[&str], cwd: &std::path::Path, home: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_medulla"))
        .args(args)
        .current_dir(cwd)
        .env("MEDULLA_HOME", home)
        .env(
            "TINYPLACE_CLAUDE_SESSIONS_DIR",
            home.join("claude-sessions"),
        )
        .env("TINYPLACE_CODEX_SESSIONS_DIR", home.join("codex-sessions"))
        .env_remove("MEDULLA_TOKEN")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("MEDULLA_BACKEND_URL")
        .output()
        .expect("the medulla binary should run")
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
fn logout_clears_the_isolated_credential_store() {
    let dir = TempDir::new().unwrap();
    let credentials = dir.path().join("credentials.json");
    std::fs::write(
        &credentials,
        r#"{"baseUrl":"http://example","jwt":"secret"}"#,
    )
    .unwrap();

    let output = run(&["logout"], dir.path(), dir.path());

    assert!(output.status.success());
    assert!(!credentials.exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Logged out"));
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
fn lessons_add_and_list_share_the_workspace_ledger() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("MEDULLA.md"),
        "Example workspace.\n\n## Lessons\n",
    )
    .unwrap();

    let added = run(
        &[
            "lessons",
            "add",
            "CI fails",
            "->",
            "inspect the first error",
        ],
        dir.path(),
        dir.path(),
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    assert!(String::from_utf8_lossy(&added.stdout).contains("Added lesson"));

    let duplicate = run(
        &[
            "lessons",
            "add",
            "CI fails",
            "->",
            "inspect the first error",
        ],
        dir.path(),
        dir.path(),
    );
    assert!(duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stdout).contains("already present"));

    let listed = run(&["lessons", "list"], dir.path(), dir.path());
    assert!(listed.status.success());
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout).trim(),
        "- when CI fails: inspect the first error"
    );
}

#[test]
fn memory_status_search_and_compile_run_fully_offline() {
    let dir = TempDir::new().unwrap();

    let status = run(&["memory", "status", "--json"], dir.path(), dir.path());
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(String::from_utf8_lossy(&status.stdout).contains("entry_count"));

    let overview = run(&["memory", "status"], dir.path(), dir.path());
    assert!(overview.status.success());

    let search = run(
        &["memory", "search", "not-present", "--json"],
        dir.path(),
        dir.path(),
    );
    assert!(search.status.success());
    assert_eq!(String::from_utf8_lossy(&search.stdout).trim(), "[]");

    let empty_search = run(&["memory", "search", "not-present"], dir.path(), dir.path());
    assert!(empty_search.status.success());
    assert!(String::from_utf8_lossy(&empty_search.stdout).contains("no matches"));

    let compile = run(&["memory", "compile"], dir.path(), dir.path());
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(String::from_utf8_lossy(&compile.stdout).contains("files"));
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

    let bad_memory = run(&["memory", "unknown"], dir.path(), dir.path());
    assert_eq!(bad_memory.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_memory.stderr).contains("unknown memory subcommand"));

    let bad_lesson = run(&["lessons", "add", "missing-arrow"], dir.path(), dir.path());
    assert_eq!(bad_lesson.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_lesson.stderr).contains("<trigger> -> <rule>"));

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
            if !announced && buffered.windows(7).any(|window| window == b"MEDULLA") {
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
            assert!(String::from_utf8_lossy(&output).contains("MEDULLA"));
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
